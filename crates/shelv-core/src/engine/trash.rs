//! Removing what the source no longer has.
//!
//! The only code in Shelv that takes a file away from someone, kept in its
//! own module for exactly that reason.
//!
//! Nothing here unlinks. A deletion is a **move** into
//! `<destination>/.shelv-trash/<timestamp>/`, keeping the file's path within
//! the backup, so undoing a mistake is a matter of moving it back. The trash
//! folder is on the destination volume, so the move is a rename rather than
//! a copy: it costs nothing, and it cannot half-succeed.
//!
//! **Why not the Recycle Bin**, which `docs/PLAN.md` §4 T4 asked for:
//! Windows commonly has the per-volume recycle bin disabled on removable
//! drives, and `IFileOperation` then deletes permanently — the precise
//! outcome the recycle bin was chosen to prevent. Backup destinations are
//! overwhelmingly removable drives. A folder Shelv controls works on every
//! filesystem, and can be tested here rather than taken on trust
//! (`docs/M1.md`).
//!
//! Nothing prunes the trash yet. Retention is M3, and a trash folder that
//! quietly emptied itself before then would be the same mistake as deleting
//! outright, only later.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::planner::Plan;
use super::stamp;
use crate::platform::PlatformFs;
use crate::{CoreError, Result};

/// The folder Shelv moves deletions into, at the root of each destination.
///
/// Leading dot so it sorts and hides out of the way, and a name nothing else
/// is likely to own. The planner skips it when indexing a destination.
pub const TRASH_DIR: &str = ".shelv-trash";

/// One file that could not be moved out of the way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct TrashFailure {
    /// Path relative to the destination root.
    pub relative: PathBuf,
    /// What went wrong, in the words the OS used.
    pub message: String,
}

/// What a deletion pass actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct TrashReport {
    /// Files moved into the trash folder.
    #[ts(type = "number")]
    pub files_trashed: u64,
    /// Bytes they occupied.
    #[ts(type = "number")]
    pub bytes_trashed: u64,
    /// The folder they went to, where anything moved. Reported so the run
    /// history can tell someone exactly where to look.
    pub folder: Option<PathBuf>,
    /// Files that could not be moved. Each one is still where it was.
    pub failures: Vec<TrashFailure>,
    /// Whether the pass stopped because it was asked to.
    pub cancelled: bool,
}

impl TrashReport {
    /// Whether every planned deletion was carried out.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && !self.cancelled
    }
}

/// Moves every file the plan marks extraneous into the destination's trash
/// folder.
///
/// `now` is a Unix timestamp; it names the folder. One folder per run, so a
/// mistake is undone by moving one directory back rather than by picking
/// files out of a pile.
///
/// Returns `Err` only when nothing could be attempted — a trash folder that
/// cannot be created. A file that fails individually is recorded and left
/// exactly where it was, and the pass continues.
///
/// `cancelled` is polled between files. There is no partial state to unwind:
/// each file has either been renamed or not.
pub fn trash_deletions(
    fs: &dyn PlatformFs,
    plan: &Plan,
    destination: &Path,
    now: i64,
    cancelled: &dyn Fn() -> bool,
) -> Result<TrashReport> {
    let mut report = TrashReport::default();
    if plan.deletions.is_empty() {
        return Ok(report);
    }

    let folder = destination.join(TRASH_DIR).join(stamp::folder_name(now));
    std::fs::create_dir_all(fs.long_path(&folder)).map_err(|e| {
        CoreError::Io(format!(
            "could not create the trash folder {}: {e}",
            folder.display()
        ))
    })?;
    report.folder = Some(folder.clone());

    for deletion in &plan.deletions {
        if cancelled() {
            report.cancelled = true;
            break;
        }

        // Defence in depth. The planner builds these by stripping the
        // destination root off a walk of it, so they are already contained;
        // this refuses anything that is not, because the one thing this
        // module must never do is move a file that was not in the backup.
        if !is_contained(&deletion.relative) {
            report.failures.push(TrashFailure {
                relative: deletion.relative.clone(),
                message: "refused: the path leaves the destination folder".to_owned(),
            });
            continue;
        }

        match move_one(fs, destination, &folder, &deletion.relative) {
            Ok(()) => {
                report.files_trashed += 1;
                report.bytes_trashed += deletion.bytes;
            }
            Err(e) => report.failures.push(TrashFailure {
                relative: deletion.relative.clone(),
                message: e.to_string(),
            }),
        }
    }

    Ok(report)
}

/// Moves one file into the trash folder, keeping its path.
fn move_one(
    fs: &dyn PlatformFs,
    destination: &Path,
    folder: &Path,
    relative: &Path,
) -> std::io::Result<()> {
    let from = fs.long_path(&destination.join(relative));
    let to = folder.join(relative);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(fs.long_path(parent))?;
    }

    // A rename within the volume, so the file is never in two places and
    // never in none. No fallback to copy-then-delete: if this fails, the
    // file stays where it is and the run reports that it did, which is the
    // only acceptable failure mode for code that removes things.
    std::fs::rename(&from, fs.long_path(&to))
}

/// Whether a relative path stays inside the folder it is relative to.
fn is_contained(relative: &Path) -> bool {
    !relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;
    use crate::engine::copier::{copy_plan_with_workers, Silent};
    use crate::engine::planner::{plan, PlanOptions};
    use crate::model::Layout;
    use crate::platform::{CaseSensitivity, SpaceInfo, VolumeInfo};

    struct FakeFs;

    impl PlatformFs for FakeFs {
        fn volumes(&self) -> Result<Vec<VolumeInfo>> {
            Ok(Vec::new())
        }

        fn volume_of(&self, _path: &Path) -> Result<VolumeInfo> {
            Err(CoreError::VolumeUnavailable("no volumes".into()))
        }

        fn space(&self, _path: &Path) -> Result<SpaceInfo> {
            Ok(SpaceInfo {
                available: 0,
                total: 0,
            })
        }

        fn long_path(&self, path: &Path) -> PathBuf {
            path.to_path_buf()
        }

        fn data_dir(&self) -> Result<PathBuf> {
            Ok(PathBuf::from("/tmp"))
        }

        fn trash(&self, _path: &Path) -> Result<()> {
            Ok(())
        }
    }

    const NOW: i64 = 1_758_526_452;
    const STAMP: &str = "2025-09-22T073412Z";

    fn options() -> PlanOptions {
        PlanOptions {
            layout: Layout::Mirror,
            excludes: Vec::new(),
            follow_symlinks: false,
            case_sensitivity: CaseSensitivity::Sensitive,
            mtime_tolerance: std::time::Duration::ZERO,
        }
    }

    fn never() -> impl Fn() -> bool {
        || false
    }

    /// One full mirror run: plan, copy, then move what the source no longer
    /// has out of the way. This is the order a real run uses.
    fn mirror(src: &Path, dst: &Path, now: i64) -> TrashReport {
        let fs = FakeFs;
        let planned = plan(&fs, src, Some(dst), &options()).unwrap();
        copy_plan_with_workers(&fs, &planned, src, dst, &Silent, 1).unwrap();
        trash_deletions(&fs, &planned, dst, now, &never()).unwrap()
    }

    #[test]
    fn a_file_the_source_no_longer_has_is_moved_rather_than_deleted() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("kept.txt"), b"kept").unwrap();
        std::fs::write(dst.path().join("removed.txt"), b"six---").unwrap();

        let report = mirror(src.path(), dst.path(), NOW);

        assert_eq!(report.files_trashed, 1);
        assert_eq!(report.bytes_trashed, 6);
        assert!(report.is_complete());

        // Gone from the backup...
        assert!(!dst.path().join("removed.txt").exists());
        // ...and recoverable, with its contents, from a folder named after
        // the run that removed it.
        let recovered = dst.path().join(TRASH_DIR).join(STAMP).join("removed.txt");
        assert_eq!(std::fs::read(&recovered).unwrap(), b"six---".to_vec());
    }

    #[test]
    fn a_trashed_file_keeps_the_path_it_had_in_the_backup() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dst.path().join("raw/2024")).unwrap();
        std::fs::write(dst.path().join("raw/2024/old.dng"), b"old").unwrap();

        mirror(src.path(), dst.path(), NOW);

        // Recovering a mistake has to be a matter of moving a folder back,
        // which only works if the structure survives the move.
        assert!(dst
            .path()
            .join(TRASH_DIR)
            .join(STAMP)
            .join("raw/2024/old.dng")
            .is_file());
    }

    #[test]
    fn a_second_run_does_not_mirror_the_trash_folder_into_itself() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("kept.txt"), b"kept").unwrap();
        std::fs::write(dst.path().join("removed.txt"), b"gone").unwrap();

        mirror(src.path(), dst.path(), NOW);

        // The trash folder is now full of files the source does not have.
        // If the planner indexed it, every one of them would look
        // extraneous, so this run would move the trash into the trash — and
        // the run after that would do it again, forever.
        let fs = FakeFs;
        let second = plan(&fs, src.path(), Some(dst.path()), &options()).unwrap();
        assert!(second.deletions.is_empty(), "{:?}", second.deletions);
        assert!(second.copies.is_empty(), "{:?}", second.copies);

        // And the first run's trash is still exactly where it was.
        assert!(dst
            .path()
            .join(TRASH_DIR)
            .join(STAMP)
            .join("removed.txt")
            .is_file());
    }

    #[test]
    fn two_runs_that_delete_get_a_folder_each() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(dst.path().join("first.txt"), b"one").unwrap();
        mirror(src.path(), dst.path(), NOW);

        std::fs::write(dst.path().join("second.txt"), b"two").unwrap();
        mirror(src.path(), dst.path(), NOW + 3_600);

        // One folder per run, so undoing a particular run means moving one
        // directory back rather than picking files out of a pile.
        assert!(dst
            .path()
            .join(TRASH_DIR)
            .join(STAMP)
            .join("first.txt")
            .is_file());
        assert!(dst
            .path()
            .join(TRASH_DIR)
            .join("2025-09-22T083412Z")
            .join("second.txt")
            .is_file());
    }

    #[test]
    fn a_snapshot_run_never_reaches_the_trash_at_all() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(dst.path().join("previous.txt"), b"earlier snapshot").unwrap();

        let fs = FakeFs;
        let mut snapshot = options();
        snapshot.layout = Layout::Snapshot;
        let planned = plan(&fs, src.path(), Some(dst.path()), &snapshot).unwrap();

        let report = trash_deletions(&fs, &planned, dst.path(), NOW, &never()).unwrap();

        // A snapshot is a record of a moment. Editing one is not a backup,
        // so the planner produces no deletions and nothing here runs.
        assert_eq!(report.files_trashed, 0);
        assert_eq!(report.folder, None);
        assert!(dst.path().join("previous.txt").exists());
        assert!(!dst.path().join(TRASH_DIR).exists());
    }

    #[test]
    fn cancelling_stops_the_pass_and_leaves_the_rest_in_place() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            std::fs::write(dst.path().join(name), b"x").unwrap();
        }

        let fs = FakeFs;
        let planned = plan(&fs, src.path(), Some(dst.path()), &options()).unwrap();
        assert_eq!(planned.deletions.len(), 4);

        // Stop after the first file has gone.
        let moved = std::sync::atomic::AtomicU64::new(0);
        let stop = || moved.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1;

        let report = trash_deletions(&fs, &planned, dst.path(), NOW, &stop).unwrap();

        assert!(report.cancelled);
        assert!(!report.is_complete());
        assert_eq!(report.files_trashed, 1);

        // Three files still in the backup, one in the trash: every file is
        // in exactly one place, because each move either happened or did
        // not.
        let remaining = std::fs::read_dir(dst.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name() != TRASH_DIR)
            .count();
        assert_eq!(remaining, 3);
    }

    #[test]
    fn a_path_that_escapes_the_destination_is_refused_rather_than_moved() {
        let dst = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("someone-elses.txt"), b"not yours").unwrap();

        // Not something the planner can produce — it builds relative paths
        // by stripping the destination root off a walk of it. This is the
        // guard for the day something else calls this.
        let mut planned = Plan::default();
        planned
            .deletions
            .push(crate::engine::planner::PlannedDeletion {
                relative: PathBuf::from("../").join(
                    outside
                        .path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                ),
                bytes: 9,
            });

        let report = trash_deletions(&FakeFs, &planned, dst.path(), NOW, &never()).unwrap();

        assert_eq!(report.files_trashed, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].message.contains("refused"));
        assert!(outside.path().join("someone-elses.txt").exists());
    }

    #[test]
    fn a_file_that_cannot_be_moved_is_recorded_and_left_where_it_is() {
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(dst.path().join("real.txt"), b"real").unwrap();

        let fs = FakeFs;
        let mut planned = Plan::default();
        // A file the plan names and the disk does not have: the ordinary
        // case of something removed between planning and running.
        planned
            .deletions
            .push(crate::engine::planner::PlannedDeletion {
                relative: PathBuf::from("vanished.txt"),
                bytes: 4,
            });
        planned
            .deletions
            .push(crate::engine::planner::PlannedDeletion {
                relative: PathBuf::from("real.txt"),
                bytes: 4,
            });

        let report = trash_deletions(&fs, &planned, dst.path(), NOW, &never()).unwrap();

        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].relative, PathBuf::from("vanished.txt"));
        // The pass carries on: one bad entry does not strand the rest.
        assert_eq!(report.files_trashed, 1);
        assert!(!report.is_complete());
    }
}

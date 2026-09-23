//! Removing old snapshots.
//!
//! The only code in Shelv that deletes without a way back. Mirror deletions
//! move to a trash folder on the same volume, which costs nothing and can be
//! undone; pruning exists precisely to reclaim that space, so a trash folder
//! here would defeat the point. What guards it instead is that every
//! condition has to hold before a single folder goes:
//!
//! * the run that just finished did **everything** it planned — not
//!   `Partial`, which may have failed to copy the very file an old snapshot
//!   is the last copy of (`docs/PLAN.md` §1.1c);
//! * the folder's name parses as one Shelv wrote, so a directory that merely
//!   shares the destination is never touched;
//! * it is not the snapshot this run just made;
//! * and the rule asked for a limit at all.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::stamp;
use crate::model::Retention;
use crate::platform::PlatformFs;

/// Seconds in a day, for `KeepDays`.
const SECONDS_PER_DAY: i64 = 86_400;

/// What a pruning pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PruneReport {
    /// Snapshot folders removed.
    #[ts(type = "number")]
    pub pruned: u64,
    /// Folders that could not be removed, with the reason. The run is not
    /// failed over one — the backup itself is already written — but it is
    /// not silent either.
    pub failures: Vec<String>,
}

/// Removes the snapshots a rule's retention no longer covers.
///
/// `keep` is the folder this run wrote, which is never a candidate however
/// the policy counts. Returns what it did rather than an error: by the time
/// this runs the backup is on the disk, and a snapshot that could not be
/// deleted is a tidiness problem, not a failed backup.
pub fn prune(
    fs: &dyn PlatformFs,
    destination: &Path,
    retention: Retention,
    keep: &Path,
    now: i64,
) -> PruneReport {
    let mut report = PruneReport::default();

    let doomed = match retention {
        // The rule asked for everything to be kept. Nothing to decide.
        Retention::Unlimited => return report,
        Retention::KeepLastN(n) => beyond_the_newest(
            fs,
            destination,
            keep,
            usize::try_from(n).unwrap_or(usize::MAX),
        ),
        Retention::KeepDays(days) => older_than(
            fs,
            destination,
            keep,
            now - i64::from(days) * SECONDS_PER_DAY,
        ),
    };

    for folder in doomed {
        match std::fs::remove_dir_all(fs.long_path(&folder)) {
            Ok(()) => report.pruned += 1,
            Err(e) => report.failures.push(format!("{}: {e}", folder.display())),
        }
    }

    report
}

/// Every snapshot Shelv wrote here, newest first, excluding `keep`.
fn snapshots(fs: &dyn PlatformFs, destination: &Path, keep: &Path) -> Vec<(i64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(fs.long_path(destination)) else {
        return Vec::new();
    };

    let mut found: Vec<(i64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            // Lossy is safe here and nowhere else in this function: a name
            // that is not valid Unicode cannot be one Shelv wrote, because
            // the format is all ASCII, so the parse below refuses it.
            #[allow(clippy::disallowed_methods)]
            let name = entry.file_name().to_string_lossy().into_owned();
            let at = stamp::parse(&name)?;
            let path = entry.path();
            (path != keep).then_some((at, path))
        })
        .collect();

    // Newest first, and by name where two share a second — the `-2` suffix
    // makes those sort in the order they were written.
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    found
}

/// The snapshots past the newest `keep_count`.
fn beyond_the_newest(
    fs: &dyn PlatformFs,
    destination: &Path,
    keep: &Path,
    keep_count: usize,
) -> Vec<PathBuf> {
    // The folder this run wrote counts towards the limit even though it is
    // never a candidate: "keep the last 3" means three including this one.
    let allowance = keep_count.saturating_sub(1);
    snapshots(fs, destination, keep)
        .into_iter()
        .skip(allowance)
        .map(|(_, path)| path)
        .collect()
}

/// The snapshots written before `cutoff`.
fn older_than(fs: &dyn PlatformFs, destination: &Path, keep: &Path, cutoff: i64) -> Vec<PathBuf> {
    snapshots(fs, destination, keep)
        .into_iter()
        .filter(|(at, _)| *at < cutoff)
        .map(|(_, path)| path)
        .collect()
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
    use crate::platform::{SpaceInfo, VolumeInfo};
    use crate::{CoreError, Result};

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

    const DAY: i64 = 86_400;
    const NOW: i64 = 1_758_526_452;

    /// A destination holding snapshots written on the given days before
    /// `NOW`, newest last. Returns the newest, as the run that just wrote it
    /// would hand over.
    fn destination(days_ago: &[i64]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let mut newest = PathBuf::new();
        for ago in days_ago {
            let at = NOW - ago * DAY;
            newest = root.path().join(stamp::folder_name(at));
            std::fs::create_dir_all(newest.join("photos")).unwrap();
            std::fs::write(newest.join("photos/one.raw"), b"x").unwrap();
        }
        (root, newest)
    }

    fn remaining(root: &Path) -> usize {
        std::fs::read_dir(root).unwrap().count()
    }

    #[test]
    fn keeping_everything_removes_nothing() {
        let (root, keep) = destination(&[9, 6, 3, 0]);
        let report = prune(&FakeFs, root.path(), Retention::Unlimited, &keep, NOW);

        assert_eq!(report.pruned, 0);
        assert_eq!(remaining(root.path()), 4);
    }

    #[test]
    fn keeping_the_last_three_leaves_three_including_this_run() {
        // "Keep the last 3" counts the snapshot just written. Counting only
        // the older ones would quietly keep four.
        let (root, keep) = destination(&[9, 6, 3, 0]);
        let report = prune(&FakeFs, root.path(), Retention::KeepLastN(3), &keep, NOW);

        assert_eq!(report.pruned, 1);
        assert_eq!(remaining(root.path()), 3);
        assert!(keep.is_dir(), "never the one just written");
        assert!(
            !root.path().join(stamp::folder_name(NOW - 9 * DAY)).exists(),
            "the oldest goes first"
        );
    }

    #[test]
    fn keeping_one_leaves_only_this_run() {
        let (root, keep) = destination(&[30, 9, 0]);
        let report = prune(&FakeFs, root.path(), Retention::KeepLastN(1), &keep, NOW);

        assert_eq!(report.pruned, 2);
        assert_eq!(remaining(root.path()), 1);
        assert!(keep.is_dir());
    }

    #[test]
    fn keeping_days_measures_from_the_name_not_the_file_dates() {
        // A copy tool has already touched every mtime in there. The folder's
        // own name is the only honest record of when the snapshot was taken.
        let (root, keep) = destination(&[40, 20, 5, 0]);
        let report = prune(&FakeFs, root.path(), Retention::KeepDays(30), &keep, NOW);

        assert_eq!(report.pruned, 1);
        assert!(!root
            .path()
            .join(stamp::folder_name(NOW - 40 * DAY))
            .exists());
        assert!(root
            .path()
            .join(stamp::folder_name(NOW - 20 * DAY))
            .is_dir());
        assert!(keep.is_dir());
    }

    #[test]
    fn a_folder_shelv_did_not_write_is_never_touched() {
        // The guard that matters most: retention deletes outright, and a
        // destination may hold anything.
        let (root, keep) = destination(&[9, 0]);
        let theirs = root.path().join("Family photos");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("wedding.jpg"), b"irreplaceable").unwrap();

        let report = prune(&FakeFs, root.path(), Retention::KeepLastN(1), &keep, NOW);

        assert_eq!(report.pruned, 1, "only the old snapshot");
        assert!(theirs.join("wedding.jpg").exists());
    }

    #[test]
    fn two_snapshots_from_the_same_second_are_ordered_by_their_suffix() {
        // The `-2` a second run in one second takes has to sort after the
        // first, or "keep the last one" keeps the wrong one.
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join(stamp::folder_name(NOW));
        let second = root.path().join(format!("{}-2", stamp::folder_name(NOW)));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let keep = root.path().join(format!("{}-3", stamp::folder_name(NOW)));
        std::fs::create_dir_all(&keep).unwrap();

        prune(&FakeFs, root.path(), Retention::KeepLastN(2), &keep, NOW);

        assert!(keep.is_dir(), "this run");
        assert!(second.is_dir(), "the one before it");
        assert!(!first.exists(), "the oldest of the three");
    }

    #[test]
    fn an_empty_destination_is_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        let keep = root.path().join(stamp::folder_name(NOW));
        let report = prune(&FakeFs, root.path(), Retention::KeepLastN(3), &keep, NOW);
        assert_eq!(report, PruneReport::default());
    }
}

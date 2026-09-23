//! One backup, start to finish.
//!
//! Plan, copy, and — for a mirror — move what the source no longer has out
//! of the way. Each of those is a module of its own; this is the order they
//! go in and the decisions that sit between them, kept in one place so that
//! a dry run and a real run cannot disagree about where files would land.
//!
//! Synchronous, and deliberately so. It knows nothing of threads, events or
//! the database: the shell runs it on a thread it owns and turns the
//! observer's calls into events (`docs/M1.md`).

use std::path::{Path, PathBuf};

use super::copier::{self, CopyObserver, CopyReport};
use super::planner::{self, Plan, PlanOptions};
use super::stamp;
use super::trash::{self, TrashReport};
use crate::model::{Layout, RunResult};
use crate::platform::PlatformFs;
use crate::safety::{self, DeletionRefusal};
use crate::Result;

/// Who is watching a run, which decides whether it may delete freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watched {
    /// Somebody pressed Backup Now, having seen the plan. The preview is
    /// the guard; nothing here second-guesses it.
    ByHand,
    /// The scheduler started it. Nobody is reading the screen, so a plan
    /// that would remove most of the backup stops instead
    /// (`safety::deletion_refusal`).
    ByNobody,
}

/// What a run wrote, and how it went.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// What the run set out to do.
    pub plan: Plan,
    /// The folder it wrote into. For a snapshot this is the timestamped
    /// folder, not the destination root, which is what the run history needs
    /// to record.
    pub target: PathBuf,
    /// What the copy did.
    pub copied: CopyReport,
    /// What was moved to the trash. Always empty for a snapshot.
    pub trashed: TrashReport,
    /// The outcome, decided by [`result`].
    pub result: RunResult,
    /// Why the run stopped before writing, where it did.
    pub refusal: Option<DeletionRefusal>,
}

/// Where a run of this layout writes, beneath a destination root.
///
/// A mirror writes into the destination folder itself; a snapshot writes
/// into a new timestamped folder beneath it and never touches one already
/// there. `now` is a Unix timestamp.
#[must_use]
pub fn target_folder(layout: Layout, destination: &Path, now: i64) -> PathBuf {
    match layout {
        Layout::Mirror => destination.to_path_buf(),
        Layout::Snapshot => destination.join(stamp::folder_name(now)),
    }
}

/// What a run of this layout compares against, given the folder it will
/// write to.
///
/// `None` means everything is an addition. A snapshot always gets `None`,
/// whatever is already on the drive: it writes a fresh tree, and comparing
/// against a previous snapshot would let a new run edit an old one — a
/// snapshot is a record of a moment, and a record that changes afterwards is
/// not one. A mirror gets `None` only on the first run, when there is
/// nothing there yet.
#[must_use]
pub fn compare_against(layout: Layout, target: &Path) -> Option<&Path> {
    (layout == Layout::Mirror && target.exists()).then_some(target)
}

/// Plans a run without writing anything.
///
/// The same call the real run makes, so a preview cannot describe a
/// different run from the one that follows it. The one difference is the
/// snapshot folder's name: a real run started in a second that already has
/// a snapshot takes the next free name, which a preview cannot know in
/// advance and which changes nothing about what is written.
pub fn plan_run(
    fs: &dyn PlatformFs,
    source: &Path,
    destination: &Path,
    options: &PlanOptions,
    now: i64,
) -> Result<(PathBuf, Plan)> {
    let target = target_folder(options.layout, destination, now);
    let plan = planner::plan(
        fs,
        source,
        compare_against(options.layout, &target),
        options,
    )?;
    Ok((target, plan))
}

/// Runs a backup.
///
/// `now` is a Unix timestamp; it names the snapshot folder and the trash
/// folder, so both belong to the same run.
///
/// Returns `Err` only when nothing could be attempted at all — an unreadable
/// source, an unwritable destination. Anything narrower is recorded in the
/// outcome and leaves [`RunOutcome::result`] as `Partial`, because a backup
/// tool that cannot tell "copied everything" from "copied what it could" is
/// worse than none.
pub fn execute(
    fs: &dyn PlatformFs,
    source: &Path,
    destination: &Path,
    options: &PlanOptions,
    now: i64,
    watched: Watched,
    observer: &dyn CopyObserver,
) -> Result<RunOutcome> {
    let target = match options.layout {
        Layout::Mirror => target_folder(options.layout, destination, now),
        // Timestamps have one-second resolution, so two runs started in the
        // same second would otherwise be handed the same folder and the
        // second would write into the first. Nothing already written is ever
        // touched, and that has to hold for a snapshot written a moment ago
        // as much as one from last year.
        Layout::Snapshot => vacant(fs, &target_folder(options.layout, destination, now))?,
    };
    let plan = planner::plan(
        fs,
        source,
        compare_against(options.layout, &target),
        options,
    )?;

    // The totals a progress bar counts towards are only knowable now, and
    // only per destination: a rule with two drives plans each as it reaches
    // it. The trait's method has a default, so a missing call here compiles
    // and shows "0 of 0 files" instead — which is why there is a test for
    // it rather than only for the copying.
    observer.planned(
        u64::try_from(plan.copies.len()).unwrap_or(u64::MAX),
        plan.bytes,
    );

    // Before anything is written, and before anything is moved. A run that
    // has decided the source looks wrong must not half-apply itself: the
    // copies are as suspect as the deletions, since both come from the same
    // reading of a source that may not be there.
    if let Some(refusal) = safety::deletion_refusal(&plan, watched == Watched::ByHand) {
        return Ok(RunOutcome {
            plan,
            target,
            copied: CopyReport::default(),
            trashed: TrashReport::default(),
            result: RunResult::Refused,
            refusal: Some(refusal),
        });
    }

    let copied = copier::copy_plan(fs, &plan, source, &target, observer)?;

    // Deletions come after the copy, and only for a mirror. After, because a
    // file that is about to be replaced should be replaced rather than
    // trashed and rewritten; only for a mirror, because the planner gives a
    // snapshot no deletions at all and this is the second place that has to
    // hold true for a file already written to be safe.
    let trashed = if options.layout.deletes_extraneous() && !copied.cancelled {
        trash::trash_deletions(fs, &plan, &target, now, &|| observer.cancelled())?
    } else {
        TrashReport::default()
    };

    Ok(RunOutcome {
        result: result(&plan, &copied, &trashed),
        plan,
        target,
        copied,
        trashed,
        refusal: None,
    })
}

/// The first free name at or after `base`, as `base`, `base-2`, `base-3`.
///
/// Gives up rather than looping forever: a hundred snapshots in one second
/// is not a backup, it is something that has gone wrong, and quietly
/// continuing would fill the drive.
fn vacant(fs: &dyn PlatformFs, base: &Path) -> Result<PathBuf> {
    if !fs.long_path(base).exists() {
        return Ok(base.to_path_buf());
    }
    for n in 2..=99 {
        let mut name = base.as_os_str().to_os_string();
        name.push(format!("-{n}"));
        let candidate = PathBuf::from(name);
        if !fs.long_path(&candidate).exists() {
            return Ok(candidate);
        }
    }
    Err(crate::CoreError::Io(format!(
        "{} already exists, and so do ninety-nine folders beside it",
        base.display()
    )))
}

/// How a run is recorded, given what it managed to do.
///
/// Cancellation wins over everything, because a cancelled run has not failed
/// and has not succeeded — it simply stopped, and calling it `Partial` would
/// put it in the same column as a run that hit unreadable files.
#[must_use]
pub fn result(plan: &Plan, copied: &CopyReport, trashed: &TrashReport) -> RunResult {
    if copied.cancelled || trashed.cancelled {
        return RunResult::Cancelled;
    }
    if plan.has_unreadable() || !copied.failures.is_empty() || !trashed.failures.is_empty() {
        return RunResult::Partial;
    }
    RunResult::Ok
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
    use crate::engine::copier::{CopyFailure, Silent};
    use crate::engine::planner::{SkipReason, Skipped};
    use crate::engine::trash::{TrashFailure, TRASH_DIR};
    use crate::platform::{CaseSensitivity, SpaceInfo, VolumeInfo};
    use crate::CoreError;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
    const LATER: i64 = NOW + 86_400;
    const LATER_STAMP: &str = "2025-09-23T073412Z";

    fn options(layout: Layout) -> PlanOptions {
        PlanOptions {
            layout,
            excludes: Vec::new(),
            follow_symlinks: false,
            case_sensitivity: CaseSensitivity::Sensitive,
            mtime_tolerance: std::time::Duration::ZERO,
        }
    }

    /// An observer that has already been cancelled when the run starts.
    struct StopAtOnce;

    impl CopyObserver for StopAtOnce {
        fn cancelled(&self) -> bool {
            true
        }
    }

    fn run(src: &Path, dst: &Path, layout: Layout, now: i64) -> RunOutcome {
        execute(
            &FakeFs,
            src,
            dst,
            &options(layout),
            now,
            Watched::ByHand,
            &Silent,
        )
        .unwrap()
    }

    #[test]
    fn a_snapshot_writes_into_a_folder_named_for_the_run() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let outcome = run(src.path(), dst.path(), Layout::Snapshot, NOW);

        assert_eq!(outcome.target, dst.path().join(STAMP));
        assert_eq!(outcome.result, RunResult::Ok);
        assert_eq!(
            std::fs::read(dst.path().join(STAMP).join("one.raw")).unwrap(),
            b"hello".to_vec()
        );
        // The destination root holds snapshots and nothing else.
        assert!(!dst.path().join("one.raw").exists());
    }

    #[test]
    fn a_second_snapshot_leaves_the_first_exactly_as_it_was() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"first version").unwrap();
        run(src.path(), dst.path(), Layout::Snapshot, NOW);

        // The source moves on: one file edited, one deleted, one added.
        std::fs::write(src.path().join("one.raw"), b"second version").unwrap();
        std::fs::write(src.path().join("new.raw"), b"new").unwrap();
        let outcome = run(src.path(), dst.path(), Layout::Snapshot, LATER);

        // This is the whole of the snapshot promise: what was written is a
        // record of a moment, and a record that changes afterwards is not
        // one.
        assert_eq!(
            std::fs::read(dst.path().join(STAMP).join("one.raw")).unwrap(),
            b"first version".to_vec()
        );
        assert!(!dst.path().join(STAMP).join("new.raw").exists());

        assert_eq!(
            std::fs::read(dst.path().join(LATER_STAMP).join("one.raw")).unwrap(),
            b"second version".to_vec()
        );
        assert_eq!(outcome.copied.files_copied, 2);
    }

    #[test]
    fn a_snapshot_copies_every_file_again_rather_than_diffing() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("unchanged.raw"), b"same").unwrap();

        run(src.path(), dst.path(), Layout::Snapshot, NOW);
        let second = run(src.path(), dst.path(), Layout::Snapshot, LATER);

        // A snapshot that skipped unchanged files would be a snapshot with
        // holes in it — restoring from it would need every earlier folder.
        assert_eq!(second.plan.copies.len(), 1);
        assert_eq!(second.copied.files_copied, 1);
        assert!(dst.path().join(LATER_STAMP).join("unchanged.raw").is_file());
    }

    #[test]
    fn a_snapshot_never_deletes_and_never_trashes() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("kept.raw"), b"kept").unwrap();
        run(src.path(), dst.path(), Layout::Snapshot, NOW);

        std::fs::remove_file(src.path().join("kept.raw")).unwrap();
        let outcome = run(src.path(), dst.path(), Layout::Snapshot, LATER);

        assert!(outcome.plan.deletions.is_empty());
        assert_eq!(outcome.trashed.files_trashed, 0);
        assert!(!dst.path().join(TRASH_DIR).exists());
        // The file is gone from the source and still in the snapshot that
        // recorded it, which is the point of keeping snapshots at all.
        assert!(dst.path().join(STAMP).join("kept.raw").is_file());
    }

    #[test]
    fn a_mirror_writes_into_the_destination_itself_and_trashes_what_went() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("kept.raw"), b"kept").unwrap();
        std::fs::write(dst.path().join("removed.raw"), b"gone").unwrap();

        let outcome = run(src.path(), dst.path(), Layout::Mirror, NOW);

        assert_eq!(outcome.target, dst.path());
        assert!(dst.path().join("kept.raw").is_file());
        assert!(!dst.path().join("removed.raw").exists());
        assert_eq!(outcome.trashed.files_trashed, 1);
        assert_eq!(outcome.result, RunResult::Ok);
    }

    #[test]
    fn two_snapshots_in_the_same_second_do_not_share_a_folder() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"first").unwrap();
        let first = run(src.path(), dst.path(), Layout::Snapshot, NOW);

        std::fs::write(src.path().join("one.raw"), b"second").unwrap();
        let second = run(src.path(), dst.path(), Layout::Snapshot, NOW);

        // A timestamp has one-second resolution. Without a distinct name the
        // second run would write into the first one's folder, and "nothing
        // already written is ever touched" would hold for last year's
        // snapshot but not for the one from a moment ago.
        assert_ne!(first.target, second.target);
        assert_eq!(first.target, dst.path().join(STAMP));
        assert_eq!(
            std::fs::read(first.target.join("one.raw")).unwrap(),
            b"first".to_vec()
        );
        assert_eq!(
            std::fs::read(second.target.join("one.raw")).unwrap(),
            b"second".to_vec()
        );
    }

    #[test]
    fn a_preview_names_the_same_work_the_run_then_does() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let (target, planned) = plan_run(
            &FakeFs,
            src.path(),
            dst.path(),
            &options(Layout::Snapshot),
            NOW,
        )
        .unwrap();
        // Nothing written by planning.
        assert!(!target.exists());

        let outcome = run(src.path(), dst.path(), Layout::Snapshot, NOW);
        assert_eq!(outcome.target, target);
        assert_eq!(outcome.plan, planned);
    }

    #[test]
    fn an_unreadable_file_makes_the_run_partial_rather_than_ok() {
        let mut plan = Plan::default();
        assert_eq!(
            result(&plan, &CopyReport::default(), &TrashReport::default()),
            RunResult::Ok
        );

        // A file the walk could not read. The run continues and the backup
        // is real, but it is not complete, and a tool that cannot tell those
        // apart is worse than none.
        plan.skipped.push(Skipped {
            relative: PathBuf::from("locked.db"),
            reason: SkipReason::Unreadable,
            detail: None,
        });
        assert_eq!(
            result(&plan, &CopyReport::default(), &TrashReport::default()),
            RunResult::Partial
        );

        // A file that could not be written, and one that could not be moved
        // out of the way, count the same.
        let failed_copy = CopyReport {
            failures: vec![CopyFailure {
                relative: PathBuf::from("a.raw"),
                message: "denied".to_owned(),
            }],
            ..CopyReport::default()
        };
        assert_eq!(
            result(&Plan::default(), &failed_copy, &TrashReport::default()),
            RunResult::Partial
        );
        let failed_trash = TrashReport {
            failures: vec![TrashFailure {
                relative: PathBuf::from("b.raw"),
                message: "denied".to_owned(),
            }],
            ..TrashReport::default()
        };
        assert_eq!(
            result(&Plan::default(), &CopyReport::default(), &failed_trash),
            RunResult::Partial
        );
    }

    #[test]
    fn a_cancelled_run_is_cancelled_rather_than_partial() {
        // Stopping is not failing and not succeeding. Recording it as
        // Partial would put it in the same column as a run that hit
        // unreadable files, which is the column someone needs to act on.
        let mut plan = Plan::default();
        plan.skipped.push(Skipped {
            relative: PathBuf::from("locked.db"),
            reason: SkipReason::Unreadable,
            detail: None,
        });
        let cancelled = CopyReport {
            cancelled: true,
            ..CopyReport::default()
        };
        assert_eq!(
            result(&plan, &cancelled, &TrashReport::default()),
            RunResult::Cancelled
        );
    }

    #[test]
    fn cancelling_a_mirror_stops_before_anything_is_trashed() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"hello").unwrap();
        std::fs::write(dst.path().join("removed.raw"), b"gone").unwrap();

        let outcome = execute(
            &FakeFs,
            src.path(),
            dst.path(),
            &options(Layout::Mirror),
            NOW,
            Watched::ByHand,
            &StopAtOnce,
        )
        .unwrap();

        assert_eq!(outcome.result, RunResult::Cancelled);
        // The deletion pass never starts, so a run stopped part-way through
        // its copy cannot remove files whose replacements were never
        // written.
        assert_eq!(outcome.trashed.files_trashed, 0);
        assert!(dst.path().join("removed.raw").exists());
        assert!(!dst.path().join(TRASH_DIR).exists());
    }

    #[test]
    fn a_folder_name_is_chosen_once_and_used_by_both_the_copy_and_the_trash() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(dst.path().join("removed.raw"), b"gone").unwrap();

        let outcome = run(src.path(), dst.path(), Layout::Mirror, NOW);

        // Both folders belong to the same run, so a run is one thing in the
        // history rather than two timestamps a second apart.
        assert_eq!(
            outcome.trashed.folder,
            Some(dst.path().join(TRASH_DIR).join(STAMP))
        );
    }

    #[test]
    fn an_unattended_run_whose_source_vanished_writes_nothing_at_all() {
        // The disaster this guards against, end to end: a source that is
        // there when the rule is written and empty when the run happens.
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        for n in 0..20 {
            std::fs::write(dst.path().join(format!("photo-{n}.raw")), b"irreplaceable").unwrap();
        }

        let outcome = execute(
            &FakeFs,
            src.path(),
            dst.path(),
            &options(Layout::Mirror),
            NOW,
            Watched::ByNobody,
            &Silent,
        )
        .unwrap();

        assert_eq!(outcome.result, RunResult::Refused);
        assert_eq!(outcome.refusal.map(|r| r.deletions), Some(20));

        // Nothing moved, nothing trashed, nothing copied. A run that has
        // decided the source looks wrong must not half-apply itself.
        assert_eq!(outcome.trashed.files_trashed, 0);
        assert!(!dst.path().join(".shelv-trash").exists());
        assert_eq!(
            std::fs::read_dir(dst.path()).unwrap().count(),
            20,
            "every file is still there"
        );
    }

    #[test]
    fn the_same_run_by_hand_goes_ahead() {
        // Backup Now has already shown the deletions and been told to
        // proceed. Refusing there would be arguing with a decision made
        // with the numbers on screen.
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        for n in 0..20 {
            std::fs::write(dst.path().join(format!("photo-{n}.raw")), b"x").unwrap();
        }

        let outcome = run(src.path(), dst.path(), Layout::Mirror, NOW);

        assert_eq!(outcome.result, RunResult::Ok);
        assert_eq!(outcome.trashed.files_trashed, 20);
    }

    /// Records what a run tells a progress bar, and when.
    #[derive(Default)]
    struct Totals {
        files: AtomicU64,
        bytes: AtomicU64,
        before_any_copy: AtomicBool,
        copied: AtomicU64,
    }

    impl CopyObserver for Totals {
        fn planned(&self, files: u64, bytes: u64) {
            self.files.store(files, Ordering::Relaxed);
            self.bytes.store(bytes, Ordering::Relaxed);
            self.before_any_copy
                .store(self.copied.load(Ordering::Relaxed) == 0, Ordering::Relaxed);
        }

        fn file_finished(&self, _relative: &Path) {
            self.copied.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn the_totals_a_progress_bar_needs_are_reported_before_the_copying() {
        // These were silently absent from M1.5 until M3.4: `planned` has a
        // default implementation, so nothing failed to compile and nothing
        // failed a test — the status bar simply read "0 of 0 files".
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"12345").unwrap();
        std::fs::write(src.path().join("two.raw"), b"678").unwrap();

        let totals = Totals::default();
        execute(
            &FakeFs,
            src.path(),
            dst.path(),
            &options(Layout::Mirror),
            NOW,
            Watched::ByHand,
            &totals,
        )
        .unwrap();

        assert_eq!(totals.files.load(Ordering::Relaxed), 2);
        assert_eq!(totals.bytes.load(Ordering::Relaxed), 8);
        assert!(
            totals.before_any_copy.load(Ordering::Relaxed),
            "a bar that learns its total half way through has already lied"
        );
    }
}

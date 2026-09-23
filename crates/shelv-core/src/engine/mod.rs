//! Planning and carrying out a backup run.
//!
//! Split so that deciding what to do is separable from doing it. The planner
//! reads and compares; nothing in it writes. That is what makes a dry run
//! possible, and it means the guards that stop a run escaping its own tree
//! can be reviewed on their own, away from the code that moves bytes
//! (`docs/M1.md`).
//!
//! The engine is synchronous. Filesystem work blocks, so an async runtime
//! would be a thread pool in a costume, and `shelv-core` would gain a
//! dependency on one for nothing. Runs happen on threads the shell spawns,
//! and progress travels back through a callback rather than a channel the
//! core has to own.

pub mod copier;
pub mod planner;
pub mod run;
pub mod stamp;
pub mod trash;

use serde::{Deserialize, Serialize};

use crate::model::{
    Destination, DestinationId, Layout, Packaging, RuleId, RuleSpec, RunId, RunResult, RunStats,
    RunTrigger,
};
use crate::platform::{PlatformFs, VolumeInfo};
use crate::store::Store;
use crate::{CoreError, Result};
use std::path::Path;

pub use copier::CopyObserver;
pub use planner::{Plan, PlanOptions};

/// What a run would do at one of a rule's destinations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanOutcome {
    /// The destination is attached and was compared against the source.
    Ready {
        /// What the run would do.
        plan: Plan,
    },
    /// Nothing was planned, and why. A destination being absent is the
    /// ordinary case for a removable drive, not an error, so it is reported
    /// per destination rather than failing the whole preview.
    Unavailable {
        /// Wording fit to show the user.
        reason: String,
    },
}

/// One destination's share of a rule's plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct DestinationPlan {
    /// Which destination this is.
    pub destination: DestinationId,
    /// What would happen there.
    pub outcome: PlanOutcome,
}

/// Works out what running a rule would do, at each of its destinations.
///
/// Reads and compares; writes nothing, which is what makes this safe to call
/// from the UI as a dry run. Paths are resolved here from the database, so
/// the caller passes an id and never a location (`docs/PLAN.md` §4, T1).
///
/// A destination whose drive is not attached yields
/// [`PlanOutcome::Unavailable`] rather than an error: a rule aimed at three
/// drives with one plugged in should still preview that one.
pub fn plan_rule(
    store: &Store,
    fs: &dyn PlatformFs,
    rule: RuleId,
    now: i64,
) -> Result<Vec<DestinationPlan>> {
    let rule = store.rule(rule)?;
    let attached = fs.volumes()?;

    let source_volume = store.volume(rule.spec.source.volume)?;
    if live_volume(&attached, &source_volume.identity).is_none() {
        return Err(CoreError::Refused(format!(
            "the source drive for \"{}\" is not attached",
            rule.spec.name
        )));
    }

    // Each destination is planned on its own terms rather than once and
    // reused: the destination volume decides case folding and mtime
    // tolerance, so the same source can diff differently against two drives.
    store
        .destinations(rule.id)?
        .into_iter()
        .map(|destination| {
            Ok(DestinationPlan {
                destination: destination.id,
                outcome: plan_destination(
                    store,
                    fs,
                    &rule.spec,
                    &attached,
                    &destination.path,
                    now,
                )?,
            })
        })
        .collect()
}

/// What one destination's run did, as the history records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RunSummary {
    /// Which destination was written to.
    pub destination: DestinationId,
    /// The history row, or `None` where the destination was skipped before
    /// a run was ever started.
    pub run: Option<RunId>,
    /// How it finished. `None` means it never started.
    pub result: Option<RunResult>,
    /// What it did.
    pub stats: RunStats,
    /// Why it did not start, in words fit to show someone.
    pub skipped: Option<String>,
}

/// What the window is told while a run is going.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RunProgress {
    /// The rule being run.
    pub rule: RuleId,
    /// The file being copied, relative to the source root. Shown, not used
    /// to resolve anything.
    pub file: String,
    /// Files finished so far.
    #[ts(type = "number")]
    pub files_done: u64,
    /// Files the plan holds in total, across the destinations planned so
    /// far.
    #[ts(type = "number")]
    pub files_total: u64,
    /// Bytes written so far.
    #[ts(type = "number")]
    pub bytes_done: u64,
    /// Bytes the plan holds in total.
    #[ts(type = "number")]
    pub bytes_total: u64,
}

/// What the window is told when a run stops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RunFinished {
    /// The rule that ran.
    pub rule: RuleId,
    /// One entry per destination, in the rule's order.
    pub summaries: Vec<RunSummary>,
    /// Why the run could not be attempted at all. The summaries are empty
    /// when this is set.
    pub error: Option<String>,
}

/// Runs a rule against every destination whose drive is attached.
///
/// Writes to disk. Everything it does is decided from the database by id —
/// no caller names a path (`docs/PLAN.md` §4, T1).
///
/// Each destination is a separate row in the run history, because each one
/// succeeds or fails on its own: a drive that was unplugged mid-run must not
/// make the drive that completed look like it failed. A destination whose
/// drive is absent is recorded as skipped rather than as a failure.
///
/// `clock` is read twice per destination, at the start and at the end, so
/// the history records how long the run actually took.
pub fn run_rule(
    store: &Store,
    fs: &dyn PlatformFs,
    rule: RuleId,
    trigger: RunTrigger,
    clock: &dyn Fn() -> i64,
    observer: &dyn CopyObserver,
) -> Result<Vec<RunSummary>> {
    let rule = store.rule(rule)?;

    // Refused up front, and loudly. A rule asking for a Zip that quietly
    // copied loose files instead would leave someone believing they had an
    // archive; packaging arrives in M4 (`docs/PLAN.md` §5).
    if rule.spec.packaging != Packaging::Files {
        return Err(CoreError::Refused(format!(
            "\"{}\" is set to write a Zip archive, which Shelv cannot do yet. \
             Set its packaging to plain files to run it now.",
            rule.spec.name
        )));
    }

    let attached = fs.volumes()?;
    let source_volume = store.volume(rule.spec.source.volume)?;
    let Some(source_live) = live_volume(&attached, &source_volume.identity) else {
        return Err(CoreError::Refused(format!(
            "the source drive for \"{}\" is not attached",
            rule.spec.name
        )));
    };
    if !source_live.is_usable() {
        return Err(CoreError::Refused(format!(
            "the source drive for \"{}\" is attached but cannot be verified as the one recorded",
            rule.spec.name
        )));
    }
    let source = source_live.mount_point.join(&rule.spec.source.relative);

    let mut summaries = Vec::new();
    for destination in store.destinations(rule.id)? {
        summaries.push(run_destination(
            store,
            fs,
            &rule.spec,
            &attached,
            &source,
            &destination,
            trigger,
            clock,
            observer,
        )?);
    }
    Ok(summaries)
}

/// Runs one destination, recording it in the history whether it worked or
/// not.
#[allow(
    clippy::too_many_arguments,
    reason = "every one of these is state the run needs and none of them group \
              into a type that would mean anything on its own"
)]
fn run_destination(
    store: &Store,
    fs: &dyn PlatformFs,
    spec: &RuleSpec,
    attached: &[VolumeInfo],
    source: &Path,
    destination: &Destination,
    trigger: RunTrigger,
    clock: &dyn Fn() -> i64,
    observer: &dyn CopyObserver,
) -> Result<RunSummary> {
    let skipped = |reason: &str| RunSummary {
        destination: destination.id,
        run: None,
        result: None,
        stats: RunStats::default(),
        skipped: Some(reason.to_owned()),
    };

    let recorded = store.volume(destination.path.volume)?;
    let Some(live) = live_volume(attached, &recorded.identity) else {
        return Ok(skipped("the drive is not attached"));
    };
    if !live.is_usable() {
        return Ok(skipped(
            "the drive is attached but cannot be verified as the one recorded",
        ));
    }

    let root = live.mount_point.join(&destination.path.relative);
    let options = PlanOptions {
        layout: spec.layout,
        excludes: spec.excludes.clone(),
        follow_symlinks: spec.follow_symlinks,
        case_sensitivity: live.case_sensitivity,
        mtime_tolerance: live.mtime_tolerance(),
    };

    let started_at = clock();
    let id = store.begin_run(destination.rule, destination.id, trigger, started_at)?;

    // Manual is the only trigger with somebody at the screen, and the only
    // one that has already shown its deletions.
    let watched = if trigger == RunTrigger::Manual {
        run::Watched::ByHand
    } else {
        run::Watched::ByNobody
    };

    match run::execute(fs, source, &root, &options, started_at, watched, observer) {
        Ok(outcome) => {
            let stats = stats_of(&outcome);
            let snapshot = (spec.layout == Layout::Snapshot).then(|| outcome.target.clone());
            // A refusal carries its reason into the history, where the point
            // of it is: somebody reading the run list later has to be able
            // to see what Shelv saw and why it stopped.
            let note = outcome.refusal.map(|refusal| refusal.to_string());
            store.finish_run(
                id,
                outcome.result,
                &stats,
                clock(),
                note.as_deref(),
                snapshot.as_deref(),
            )?;
            Ok(RunSummary {
                destination: destination.id,
                run: Some(id),
                result: Some(outcome.result),
                stats,
                skipped: None,
            })
        }
        Err(e) => {
            // A run that could not be attempted still belongs in the
            // history. A rule whose last result is simply missing reads as
            // "never run", which is the one thing it is not.
            let message = e.to_string();
            store.finish_run(
                id,
                RunResult::Failed,
                &RunStats::default(),
                clock(),
                Some(&message),
                None,
            )?;
            Ok(RunSummary {
                destination: destination.id,
                run: Some(id),
                result: Some(RunResult::Failed),
                stats: RunStats::default(),
                skipped: None,
            })
        }
    }
}

/// Turns an outcome into the counters the history stores.
fn stats_of(outcome: &run::RunOutcome) -> RunStats {
    // Excluded files are a choice the rule made, not something that went
    // wrong, so they are not counted as skipped — a run that excluded a
    // thousand temp files by design must not read as a run that failed to
    // copy a thousand files.
    let unreadable = outcome
        .plan
        .skipped
        .iter()
        .filter(|s| s.reason != planner::SkipReason::Excluded)
        .count();

    RunStats {
        files_copied: outcome.copied.files_copied,
        files_skipped: u64::try_from(unreadable + outcome.copied.failures.len())
            .unwrap_or(u64::MAX),
        files_deleted: outcome.trashed.files_trashed,
        bytes_copied: outcome.copied.bytes_copied,
        ..RunStats::default()
    }
}

/// Plans one destination, or says why it cannot be planned.
fn plan_destination(
    store: &Store,
    fs: &dyn PlatformFs,
    spec: &RuleSpec,
    attached: &[VolumeInfo],
    destination: &crate::model::VolumePath,
    now: i64,
) -> Result<PlanOutcome> {
    let recorded = store.volume(destination.volume)?;
    let Some(live) = live_volume(attached, &recorded.identity) else {
        return Ok(PlanOutcome::Unavailable {
            reason: "the drive is not attached".into(),
        });
    };
    if !live.is_usable() {
        return Ok(PlanOutcome::Unavailable {
            reason: "the drive is attached but cannot be verified as the one recorded".into(),
        });
    }

    let source_volume = store.volume(spec.source.volume)?;
    let Some(source_live) = live_volume(attached, &source_volume.identity) else {
        return Ok(PlanOutcome::Unavailable {
            reason: "the source drive is not attached".into(),
        });
    };
    let source = source_live.mount_point.join(&spec.source.relative);
    let root = live.mount_point.join(&destination.relative);

    let options = PlanOptions {
        layout: spec.layout,
        excludes: spec.excludes.clone(),
        follow_symlinks: spec.follow_symlinks,
        case_sensitivity: live.case_sensitivity,
        mtime_tolerance: live.mtime_tolerance(),
    };

    // Through the same call the real run makes, so a preview can never
    // describe a different run from the one that follows it. `now` only
    // names the snapshot folder a future run would create; the preview says
    // what would be written, not when.
    run::plan_run(fs, &source, &root, &options, now).map(|(_, plan)| PlanOutcome::Ready { plan })
}

/// The attached volume matching a recorded identity, if it is there.
fn live_volume<'a>(
    attached: &'a [VolumeInfo],
    identity: &crate::platform::VolumeIdentity,
) -> Option<&'a VolumeInfo> {
    attached.iter().find(|v| &v.identity == identity)
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
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::engine::copier::Silent;
    use crate::model::{
        Layout, Packaging, PlaceholderPolicy, Retention, RuleSpec, Schedule, VolumePath,
    };
    use crate::platform::{
        CaseSensitivity, DriveType, SpaceInfo, VolumeIdentity, VolumeIdentityKind,
    };

    /// A platform whose volumes are temp directories.
    struct FakeFs {
        volumes: Vec<VolumeInfo>,
    }

    impl PlatformFs for FakeFs {
        fn volumes(&self) -> Result<Vec<VolumeInfo>> {
            Ok(self.volumes.clone())
        }

        fn volume_of(&self, path: &Path) -> Result<VolumeInfo> {
            self.volumes
                .iter()
                .filter(|v| path.starts_with(&v.mount_point))
                .max_by_key(|v| v.mount_point.as_os_str().len())
                .cloned()
                .ok_or_else(|| CoreError::VolumeUnavailable("no such volume".into()))
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

    fn volume(mount: &Path, id: &str) -> VolumeInfo {
        VolumeInfo {
            identity: VolumeIdentity {
                kind: VolumeIdentityKind::LinuxFsUuid,
                value: id.to_owned(),
            },
            mount_point: mount.to_path_buf(),
            serial: None,
            label: Some(id.to_owned()),
            filesystem: Some("ext4".to_owned()),
            drive_type: DriveType::Removable,
            case_sensitivity: CaseSensitivity::Sensitive,
            is_sync_root: false,
        }
    }

    fn spec(source: VolumePath, layout: Layout) -> RuleSpec {
        RuleSpec {
            name: "Photos".to_owned(),
            enabled: true,
            source,
            layout,
            packaging: Packaging::Files,
            retention: Retention::Unlimited,
            schedule: Schedule::Manual,
            run_on_connect: false,
            placeholders: PlaceholderPolicy::Hydrate,
            hydrate_budget_bytes: None,
            follow_symlinks: false,
            excludes: Vec::new(),
        }
    }

    /// A store holding one rule from `src` to `dst`, and a matching platform.
    fn fixture(src: &Path, dst: &Path, layout: Layout) -> (Store, FakeFs, RuleId) {
        let store = Store::open_in_memory().unwrap();
        let source_volume = store
            .upsert_volume(&volume(src, "source-drive"), Some(0))
            .unwrap();
        let dest_volume = store
            .upsert_volume(&volume(dst, "backup-drive"), Some(0))
            .unwrap();

        let source = VolumePath {
            volume: source_volume,
            relative: PathBuf::new(),
        };
        let rule = store.create_rule(&spec(source, layout), 0).unwrap();
        store
            .add_destination(
                rule,
                &VolumePath {
                    volume: dest_volume,
                    relative: PathBuf::new(),
                },
                0,
            )
            .unwrap();

        let fs = FakeFs {
            volumes: vec![volume(src, "source-drive"), volume(dst, "backup-drive")],
        };
        (store, fs, rule)
    }

    const NOW: i64 = 1_758_526_452;

    fn plan_of(outcome: &PlanOutcome) -> &Plan {
        match outcome {
            PlanOutcome::Ready { plan } => plan,
            PlanOutcome::Unavailable { reason } => panic!("expected a plan, got: {reason}"),
        }
    }

    #[test]
    fn a_rule_is_planned_from_its_id_alone() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        let plans = plan_rule(&store, &fs, rule, NOW).unwrap();

        assert_eq!(plans.len(), 1);
        let plan = plan_of(&plans[0].outcome);
        assert_eq!(plan.copies.len(), 1);
        assert_eq!(plan.copies[0].relative, PathBuf::from("one.raw"));
        assert_eq!(plan.bytes, 5);
    }

    #[test]
    fn planning_writes_nothing_to_either_side() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("one.raw"), b"hello").unwrap();
        fs::write(dst.path().join("gone.raw"), b"old").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        let plans = plan_rule(&store, &fs, rule, NOW).unwrap();

        // The plan says the destination file would go...
        assert_eq!(plan_of(&plans[0].outcome).deletions.len(), 1);
        // ...and it is still there, because planning only reads.
        assert!(dst.path().join("gone.raw").exists());
        assert!(!dst.path().join("one.raw").exists());
    }

    #[test]
    fn a_destination_whose_drive_is_absent_is_reported_rather_than_failing_the_preview() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let (store, mut fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);

        // Unplug the backup drive, leaving the rule pointing at it.
        fs.volumes.retain(|v| v.identity.value != "backup-drive");

        let plans = plan_rule(&store, &fs, rule, NOW).unwrap();
        assert_eq!(plans.len(), 1);
        assert!(
            matches!(&plans[0].outcome, PlanOutcome::Unavailable { reason } if reason.contains("not attached")),
            "{:?}",
            plans[0].outcome
        );
    }

    #[test]
    fn a_snapshot_never_compares_against_what_is_already_there() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("one.raw"), b"hello").unwrap();
        // A previous snapshot, byte for byte identical in the destination
        // root. A mirror would call this file unchanged; a snapshot writes a
        // fresh tree and must copy it again rather than edit history.
        fs::write(dst.path().join("one.raw"), b"hello").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Snapshot);
        let plan = plan_rule(&store, &fs, rule, NOW).unwrap();
        let plan = plan_of(&plan[0].outcome);

        assert_eq!(plan.copies.len(), 1, "a snapshot copies everything");
        assert!(plan.deletions.is_empty(), "a snapshot never deletes");
    }

    #[test]
    fn an_absent_source_drive_refuses_the_whole_preview() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let (store, mut fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);

        fs.volumes.retain(|v| v.identity.value != "source-drive");

        let error = plan_rule(&store, &fs, rule, NOW).expect_err("nothing can be planned");
        assert!(matches!(error, CoreError::Refused(_)), "{error:?}");
    }

    #[test]
    fn running_a_rule_copies_to_every_attached_destination_and_records_each() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        let summaries = run_rule(&store, &fs, rule, RunTrigger::Manual, &|| NOW, &Silent).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].result, Some(RunResult::Ok));
        assert_eq!(summaries[0].stats.files_copied, 1);
        assert_eq!(summaries[0].stats.bytes_copied, 5);
        assert_eq!(
            fs::read(dst.path().join("one.raw")).unwrap(),
            b"hello".to_vec()
        );

        // The history is what the rule table reads, so a run that happened
        // and left no row would show as "never backed up".
        let last = store.last_run(rule).unwrap().unwrap();
        assert_eq!(last.result, Some(RunResult::Ok));
        assert_eq!(last.stats.files_copied, 1);
        assert!(last.finished_at.is_some());
    }

    #[test]
    fn a_destination_whose_drive_is_absent_is_skipped_rather_than_recorded_as_a_failure() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let (store, mut fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        fs.volumes.retain(|v| v.identity.value != "backup-drive");

        let summaries = run_rule(&store, &fs, rule, RunTrigger::Manual, &|| NOW, &Silent).unwrap();

        // An unplugged backup drive is the ordinary case, not a failure, and
        // recording it as one would fill the history with red for a rule
        // that is working exactly as intended.
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].result, None);
        assert!(summaries[0].skipped.is_some());
        assert!(store.last_run(rule).unwrap().is_none());
    }

    #[test]
    fn a_rule_set_to_write_a_zip_is_refused_rather_than_quietly_copying_loose_files() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        let mut spec = store.rule(rule).unwrap().spec;
        spec.packaging = Packaging::ZipDeflate;
        store.update_rule(rule, &spec).unwrap();

        let error = run_rule(&store, &fs, rule, RunTrigger::Manual, &|| NOW, &Silent)
            .expect_err("packaging is M4");
        assert!(matches!(error, CoreError::Refused(_)), "{error:?}");

        // Nothing was written, and nothing claims to have been: someone who
        // asked for an archive must not end up with loose files and a green
        // result.
        assert!(!dst.path().join("one.raw").exists());
        assert!(store.last_run(rule).unwrap().is_none());
    }

    #[test]
    fn a_snapshot_run_records_the_folder_it_wrote() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Snapshot);
        run_rule(&store, &fs, rule, RunTrigger::Manual, &|| NOW, &Silent).unwrap();

        // Which folder a snapshot went to is the only way back to the files,
        // so the history has to carry it.
        let last = store.last_run(rule).unwrap().unwrap();
        let recorded = last.snapshot_path.unwrap();
        assert!(recorded.join("one.raw").is_file(), "{}", recorded.display());
    }

    #[test]
    fn excluded_files_do_not_count_as_skipped_in_the_history() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("keep.raw"), b"keep").unwrap();
        fs::write(src.path().join("scratch.tmp"), b"tmp").unwrap();

        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        let mut spec = store.rule(rule).unwrap().spec;
        spec.excludes = vec!["*.tmp".to_owned()];
        store.update_rule(rule, &spec).unwrap();

        run_rule(&store, &fs, rule, RunTrigger::Manual, &|| NOW, &Silent).unwrap();

        // An exclusion is the rule working, not the rule failing. Counting
        // it as skipped would make a rule that ignores a thousand temp files
        // by design read as one that failed to copy a thousand files.
        let last = store.last_run(rule).unwrap().unwrap();
        assert_eq!(last.stats.files_copied, 1);
        assert_eq!(last.stats.files_skipped, 0);
        assert_eq!(last.result, Some(RunResult::Ok));
    }

    #[test]
    fn a_run_records_how_long_it_took() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let (store, fs, rule) = fixture(src.path(), dst.path(), Layout::Mirror);

        // A clock that moves, so started_at and finished_at cannot both come
        // from one reading.
        let tick = std::sync::atomic::AtomicI64::new(NOW);
        let clock = || tick.fetch_add(5, std::sync::atomic::Ordering::Relaxed);

        run_rule(&store, &fs, rule, RunTrigger::Manual, &clock, &Silent).unwrap();

        let last = store.last_run(rule).unwrap().unwrap();
        assert_eq!(last.started_at, NOW);
        assert_eq!(last.finished_at, Some(NOW + 5));
    }

    #[test]
    fn a_scheduled_run_that_would_empty_the_backup_is_recorded_as_refused() {
        // Through the store, because the history is what someone reads the
        // next morning: the result has to be distinguishable from a failure
        // and it has to say what Shelv saw.
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        for n in 0..20 {
            fs::write(dst.path().join(format!("photo-{n}.raw")), b"irreplaceable").unwrap();
        }

        let (store, platform, rule) = fixture(src.path(), dst.path(), Layout::Mirror);
        let summaries = run_rule(
            &store,
            &platform,
            rule,
            RunTrigger::Schedule,
            &|| NOW,
            &Silent,
        )
        .unwrap();

        assert_eq!(summaries[0].result, Some(RunResult::Refused));

        let last = store.last_run(rule).unwrap().unwrap();
        assert_eq!(last.result, Some(RunResult::Refused));
        assert!(
            last.error
                .unwrap_or_default()
                .contains("20 of the 20 files"),
            "the history has to say what it saw"
        );
        assert_eq!(fs::read_dir(dst.path()).unwrap().count(), 20);
    }
}

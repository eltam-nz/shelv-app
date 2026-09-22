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
pub mod stamp;
pub mod trash;

use serde::{Deserialize, Serialize};

use crate::model::{DestinationId, Layout, RuleId};
use crate::platform::{PlatformFs, VolumeInfo};
use crate::store::Store;
use crate::{CoreError, Result};

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
pub fn plan_rule(store: &Store, fs: &dyn PlatformFs, rule: RuleId) -> Result<Vec<DestinationPlan>> {
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
                outcome: plan_destination(store, fs, &rule.spec, &attached, &destination.path)?,
            })
        })
        .collect()
}

/// Plans one destination, or says why it cannot be planned.
fn plan_destination(
    store: &Store,
    fs: &dyn PlatformFs,
    spec: &crate::model::RuleSpec,
    attached: &[VolumeInfo],
    destination: &crate::model::VolumePath,
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

    // A snapshot writes a fresh timestamped tree, so there is nothing to
    // compare against even when the destination folder is full of previous
    // snapshots — comparing would let a new run edit an old one.
    let compare_against = if spec.layout == Layout::Snapshot || !root.exists() {
        None
    } else {
        Some(root.as_path())
    };

    planner::plan(fs, &source, compare_against, &options).map(|plan| PlanOutcome::Ready { plan })
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
    use crate::model::{Packaging, PlaceholderPolicy, Retention, RuleSpec, Schedule, VolumePath};
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
            catch_up: false,
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
        let plans = plan_rule(&store, &fs, rule).unwrap();

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
        let plans = plan_rule(&store, &fs, rule).unwrap();

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

        let plans = plan_rule(&store, &fs, rule).unwrap();
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
        let plan = plan_rule(&store, &fs, rule).unwrap();
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

        let error = plan_rule(&store, &fs, rule).expect_err("nothing can be planned");
        assert!(matches!(error, CoreError::Refused(_)), "{error:?}");
    }
}

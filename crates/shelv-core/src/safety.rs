//! Guards that refuse a rule which could destroy data.
//!
//! These run when a rule is saved, not when it runs. That is deliberate: a
//! rule is a standing instruction, and by the time the engine acts on one the
//! person who wrote it is usually asleep. Catching "the destination is inside
//! the source" at save time turns a corrupted backup into a form error.
//!
//! Everything here refuses by default. A case this module cannot reason about
//! is rejected rather than allowed, because the cost of a wrong rejection is a
//! confused user and the cost of a wrong acceptance is lost files.
//!
//! One guard runs later than the rest: [`deletion_refusal`] weighs what a
//! run is about to remove, which cannot be known until the plan exists. It
//! is here rather than in the engine because it is the same kind of thing —
//! a refusal on behalf of somebody who is not watching.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::engine::planner::Plan;
use crate::model::{RuleSpec, VolumePath};
use crate::view::VolumeStatus;

/// Why a rule was refused.
///
/// Carries enough detail for the UI to point at the field that caused it,
/// rather than showing one opaque "invalid rule" message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleProblem {
    /// The rule has no name, so it cannot be identified in the table.
    EmptyName,

    /// The source and a destination are the same place. Backing a folder up
    /// onto itself either does nothing or, in mirror mode, deletes it.
    SourceIsDestination {
        /// Which destination, by position in the list.
        destination_index: usize,
    },

    /// A destination is inside the source tree. Each run would copy the
    /// previous run's output, so the backup grows without bound and the
    /// source tree fills the disk.
    DestinationInsideSource {
        /// Which destination, by position in the list.
        destination_index: usize,
    },

    /// The source is inside a destination tree. In mirror mode with
    /// deletions the source itself becomes a deletion candidate.
    SourceInsideDestination {
        /// Which destination, by position in the list.
        destination_index: usize,
    },

    /// Two destinations name the same place, so the run would race itself.
    DuplicateDestination {
        /// The later of the two, by position in the list.
        destination_index: usize,
    },

    /// A destination writes to the root of a volume rather than a folder on
    /// it. In mirror mode with deletions that puts every other file on the
    /// drive in scope.
    DestinationIsVolumeRoot {
        /// Which destination, by position in the list.
        destination_index: usize,
    },

    /// A destination is a system location. Nothing good comes of a backup
    /// tool writing into Windows or Program Files.
    DestinationIsSystemPath {
        /// Which destination, by position in the list.
        destination_index: usize,
        /// The path that matched, for the message.
        path: String,
    },

    /// The rule has nowhere to write.
    NoDestinations,

    /// The source volume cannot be used — unidentifiable, a network share,
    /// or otherwise refused.
    SourceVolumeUnusable {
        /// Why, in the same words the table uses.
        reason: String,
    },

    /// A destination volume cannot be used.
    DestinationVolumeUnusable {
        /// Which destination, by position in the list.
        destination_index: usize,
        /// Why, in the same words the table uses.
        reason: String,
    },

    /// A relative path escaped its volume, via `..` or an absolute component.
    /// Should be impossible through the folder picker; refused anyway, since
    /// the database is editable by hand.
    PathEscapesVolume {
        /// The offending path.
        path: String,
    },
}

impl RuleProblem {
    /// The destination this problem concerns, if it concerns one.
    #[must_use]
    pub const fn destination_index(&self) -> Option<usize> {
        match self {
            Self::SourceIsDestination { destination_index }
            | Self::DestinationInsideSource { destination_index }
            | Self::SourceInsideDestination { destination_index }
            | Self::DuplicateDestination { destination_index }
            | Self::DestinationIsVolumeRoot { destination_index }
            | Self::DestinationIsSystemPath {
                destination_index, ..
            }
            | Self::DestinationVolumeUnusable {
                destination_index, ..
            } => Some(*destination_index),
            Self::EmptyName
            | Self::NoDestinations
            | Self::SourceVolumeUnusable { .. }
            | Self::PathEscapesVolume { .. } => None,
        }
    }
}

/// Relative paths that must not be used as a destination on a system volume.
///
/// Matched case-insensitively against the leading components, so
/// `Windows/System32/whatever` is caught by `Windows`.
const SYSTEM_PREFIXES: &[&str] = &[
    "Windows",
    "Program Files",
    "Program Files (x86)",
    "ProgramData",
    "System Volume Information",
    "$Recycle.Bin",
    "bin",
    "boot",
    "dev",
    "etc",
    "lib",
    "proc",
    "sbin",
    "sys",
    "usr",
    "var",
];

/// Whether a volume-relative path stays inside its volume.
///
/// Rejects `..` and any rooted component. The folder picker cannot produce
/// either, but the database can be edited by hand and the engine must not
/// trust what it reads back.
fn is_contained(relative: &Path) -> bool {
    relative.components().all(|c| match c {
        Component::Normal(_) | Component::CurDir => true,
        Component::ParentDir | Component::RootDir | Component::Prefix(_) => false,
    })
}

/// Whether `inner` is the same as, or beneath, `outer` on the same volume.
fn is_within(inner: &VolumePath, outer: &VolumePath, case_insensitive: bool) -> bool {
    if inner.volume != outer.volume {
        return false;
    }
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(part) => {
                    let s = part.to_string_lossy().into_owned();
                    Some(if case_insensitive {
                        s.to_lowercase()
                    } else {
                        s
                    })
                }
                _ => None,
            })
            .collect()
    };
    let inner_parts = norm(&inner.relative);
    let outer_parts = norm(&outer.relative);
    // A volume root (no components) contains everything on that volume.
    inner_parts.len() >= outer_parts.len() && inner_parts.starts_with(&outer_parts)
}

/// Whether a relative path is empty, i.e. the volume root itself.
fn is_volume_root(relative: &Path) -> bool {
    !relative
        .components()
        .any(|c| matches!(c, Component::Normal(_)))
}

/// Whether a relative path starts with a known system directory.
fn system_prefix(relative: &Path) -> Option<String> {
    let first = relative.components().find_map(|c| match c {
        Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
        _ => None,
    })?;
    SYSTEM_PREFIXES
        .iter()
        .find(|p| first.eq_ignore_ascii_case(p))
        .map(|p| (*p).to_owned())
}

/// Human wording for why a volume cannot be used, matching the table.
fn unusable_reason(status: &VolumeStatus) -> Option<String> {
    use crate::view::Availability;
    match status.availability {
        // A disconnected volume is fine to configure against — that is the
        // entire point of a removable-drive backup. It is refused at run
        // time, not at save time.
        Availability::Available | Availability::Disconnected => None,
        Availability::Refused => Some(
            "Shelv does not back up to this kind of volume — network shares, optical media \
             and unclassifiable drives are excluded"
                .to_owned(),
        ),
        Availability::Unverifiable => Some(
            "this drive is connected, but the system reports nothing that identifies it \
             across reconnections, so Shelv cannot tell it apart from a different drive \
             plugged into the same place"
                .to_owned(),
        ),
        Availability::IdentityMismatch => Some(
            "a drive is mounted where this one used to be, but it is not the same drive".to_owned(),
        ),
    }
}

/// Checks a rule, returning every problem found.
///
/// All problems are returned rather than just the first, so the editor can
/// show them together instead of revealing them one save at a time.
///
/// `case_insensitive` should come from the source volume's
/// [`CaseSensitivity`](crate::platform::CaseSensitivity): on NTFS,
/// `Backups/out` and `backups/OUT` are the same folder, and a containment
/// check that ignores that is trivially bypassed.
#[must_use]
pub fn check_rule(
    spec: &RuleSpec,
    destinations: &[VolumePath],
    source_status: Option<&VolumeStatus>,
    destination_statuses: &[Option<&VolumeStatus>],
    case_insensitive: bool,
) -> Vec<RuleProblem> {
    let mut problems = Vec::new();

    if spec.name.trim().is_empty() {
        problems.push(RuleProblem::EmptyName);
    }

    if !is_contained(&spec.source.relative) {
        problems.push(RuleProblem::PathEscapesVolume {
            path: spec.source.relative.display().to_string(),
        });
    }

    if let Some(reason) = source_status.and_then(unusable_reason) {
        problems.push(RuleProblem::SourceVolumeUnusable { reason });
    }

    if destinations.is_empty() {
        problems.push(RuleProblem::NoDestinations);
    }

    for (index, destination) in destinations.iter().enumerate() {
        if !is_contained(&destination.relative) {
            problems.push(RuleProblem::PathEscapesVolume {
                path: destination.relative.display().to_string(),
            });
            // Containment underpins every check below, so skip them rather
            // than reason about a path that has already escaped.
            continue;
        }

        if let Some(reason) = destination_statuses
            .get(index)
            .copied()
            .flatten()
            .and_then(unusable_reason)
        {
            problems.push(RuleProblem::DestinationVolumeUnusable {
                destination_index: index,
                reason,
            });
        }

        if is_volume_root(&destination.relative) {
            problems.push(RuleProblem::DestinationIsVolumeRoot {
                destination_index: index,
            });
        }

        if let Some(path) = system_prefix(&destination.relative) {
            problems.push(RuleProblem::DestinationIsSystemPath {
                destination_index: index,
                path,
            });
        }

        let same = destination.volume == spec.source.volume
            && is_within(destination, &spec.source, case_insensitive)
            && is_within(&spec.source, destination, case_insensitive);

        if same {
            problems.push(RuleProblem::SourceIsDestination {
                destination_index: index,
            });
        } else if is_within(destination, &spec.source, case_insensitive) {
            problems.push(RuleProblem::DestinationInsideSource {
                destination_index: index,
            });
        } else if is_within(&spec.source, destination, case_insensitive) {
            problems.push(RuleProblem::SourceInsideDestination {
                destination_index: index,
            });
        }

        // Compare against earlier destinations only, so a duplicate pair
        // produces one problem rather than two.
        if destinations.iter().take(index).any(|earlier| {
            earlier.volume == destination.volume
                && is_within(earlier, destination, case_insensitive)
                && is_within(destination, earlier, case_insensitive)
        }) {
            problems.push(RuleProblem::DuplicateDestination {
                destination_index: index,
            });
        }
    }

    problems
}

// -- the unattended deletion guard ---------------------------------------

/// The share of a destination that may disappear in one unattended run
/// before Shelv stops and asks.
///
/// A quarter. There is no principled number here, only a judgement: a
/// routine day's deletions are a handful of files out of thousands, and the
/// event this exists to catch — a source that mounted empty, or was renamed
/// — takes nearly all of them. A quarter sits far above the first and far
/// below the second.
const MAX_UNATTENDED_DELETION_SHARE: u64 = 4;

/// Below this many deletions, the share is not considered at all.
///
/// Both halves are needed. A percentage alone refuses a two-file folder
/// losing one file, which is ordinary; a count alone never triggers on a
/// large backup, where losing a quarter of it is exactly the disaster.
const MIN_UNATTENDED_DELETIONS: u64 = 8;

/// What an unattended run was about to remove, when that looked wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct DeletionRefusal {
    /// Files the plan would have removed from the destination.
    #[ts(type = "number")]
    pub deletions: u64,
    /// Files that were there.
    #[ts(type = "number")]
    pub destination_files: u64,
}

impl std::fmt::Display for DeletionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this run would remove {} of the {} files at the destination, \
             which looks less like a backup than like a source that is not \
             there. Nothing was changed. Run it by hand to see the full list \
             and decide.",
            self.deletions, self.destination_files
        )
    }
}

/// Whether an unattended run should stop rather than delete.
///
/// M1 only ever deleted with somebody watching a preview. A scheduled run
/// removes that person, and a mirror faithfully reproduces a source that
/// has gone missing by emptying the backup of it. This is the check that
/// stands in for the reader who is not there.
///
/// `attended` runs — Backup Now — always return `None`: the preview is the
/// guard there, and someone who has read it and pressed the button has
/// already made this decision.
#[must_use]
pub fn deletion_refusal(plan: &Plan, attended: bool) -> Option<DeletionRefusal> {
    if attended {
        return None;
    }

    let deletions = u64::try_from(plan.deletions.len()).unwrap_or(u64::MAX);
    if deletions < MIN_UNATTENDED_DELETIONS {
        return None;
    }

    // Multiplication rather than a division or a float: an exact comparison,
    // and no rounding to argue about at the boundary.
    if deletions * MAX_UNATTENDED_DELETION_SHARE <= plan.destination_files {
        return None;
    }

    Some(DeletionRefusal {
        deletions,
        destination_files: plan.destination_files,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{Layout, Packaging, PlaceholderPolicy, Retention, Schedule, VolumeId};

    fn path(volume: i64, relative: &str) -> VolumePath {
        VolumePath {
            volume: VolumeId(volume),
            relative: PathBuf::from(relative),
        }
    }

    fn spec(source: VolumePath) -> RuleSpec {
        RuleSpec {
            name: "Photos".to_owned(),
            enabled: true,
            source,
            layout: Layout::Mirror,
            packaging: Packaging::Files,
            retention: Retention::Unlimited,
            schedule: Schedule::Manual,
            run_on_connect: true,
            placeholders: PlaceholderPolicy::Hydrate,
            hydrate_budget_bytes: None,
            follow_symlinks: false,
            excludes: Vec::new(),
        }
    }

    fn check(s: &RuleSpec, dests: &[VolumePath]) -> Vec<RuleProblem> {
        let statuses = vec![None; dests.len()];
        check_rule(s, dests, None, &statuses, false)
    }

    #[test]
    fn a_reasonable_rule_is_accepted() {
        let s = spec(path(1, "Pictures/Lightroom"));
        assert_eq!(check(&s, &[path(2, "Backups/Lightroom")]), Vec::new());
    }

    #[test]
    fn a_destination_inside_the_source_is_refused() {
        // Each run would copy the previous run's output back into itself.
        let s = spec(path(1, "Pictures"));
        assert_eq!(
            check(&s, &[path(1, "Pictures/Backups")]),
            vec![RuleProblem::DestinationInsideSource {
                destination_index: 0
            }]
        );
    }

    #[test]
    fn a_source_inside_a_destination_is_refused() {
        // In mirror mode with deletions the source becomes a candidate for
        // deletion by its own rule.
        let s = spec(path(1, "Backups/Pictures"));
        assert_eq!(
            check(&s, &[path(1, "Backups")]),
            vec![RuleProblem::SourceInsideDestination {
                destination_index: 0
            }]
        );
    }

    #[test]
    fn backing_a_folder_up_onto_itself_is_refused() {
        let s = spec(path(1, "Pictures"));
        assert_eq!(
            check(&s, &[path(1, "Pictures")]),
            vec![RuleProblem::SourceIsDestination {
                destination_index: 0
            }]
        );
    }

    #[test]
    fn containment_respects_case_insensitive_volumes() {
        // On NTFS these are the same folder. A case-sensitive comparison
        // would wave the rule through and the engine would then recurse.
        let s = spec(path(1, "Pictures"));
        let dests = [path(1, "pictures/BACKUPS")];

        assert_eq!(check_rule(&s, &dests, None, &[None], false), Vec::new());
        assert_eq!(
            check_rule(&s, &dests, None, &[None], true),
            vec![RuleProblem::DestinationInsideSource {
                destination_index: 0
            }]
        );
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_inside() {
        // "Pictures2" starts with "Pictures" as a string but is not beneath
        // it. Comparing raw strings rather than path components would refuse
        // a perfectly good rule.
        let s = spec(path(1, "Pictures"));
        assert_eq!(check(&s, &[path(1, "Pictures2/Backups")]), Vec::new());
    }

    #[test]
    fn the_same_relative_path_on_another_volume_is_fine() {
        // The commonest real rule: back Pictures up to Pictures on a
        // different drive.
        let s = spec(path(1, "Pictures"));
        assert_eq!(check(&s, &[path(2, "Pictures")]), Vec::new());
    }

    #[test]
    fn a_volume_root_destination_is_refused() {
        let s = spec(path(1, "Pictures"));
        assert_eq!(
            check(&s, &[path(2, "")]),
            vec![RuleProblem::DestinationIsVolumeRoot {
                destination_index: 0
            }]
        );
    }

    #[test]
    fn system_directories_are_refused_as_destinations() {
        let s = spec(path(1, "Pictures"));
        for dir in [
            "Windows/Temp",
            "Program Files/Shelv",
            "etc/shelv",
            "$Recycle.Bin",
        ] {
            let problems = check(&s, &[path(2, dir)]);
            assert!(
                problems
                    .iter()
                    .any(|p| matches!(p, RuleProblem::DestinationIsSystemPath { .. })),
                "{dir} should be refused, got {problems:?}"
            );
        }
    }

    #[test]
    fn a_path_escaping_its_volume_is_refused() {
        // Unreachable through the folder picker, but the database is
        // editable by hand and the engine must not trust it.
        let s = spec(path(1, "Pictures"));
        assert_eq!(
            check(&s, &[path(2, "../../elsewhere")]),
            vec![RuleProblem::PathEscapesVolume {
                path: "../../elsewhere".to_owned()
            }]
        );
    }

    #[test]
    fn a_rule_with_no_destinations_is_refused() {
        let s = spec(path(1, "Pictures"));
        assert_eq!(check(&s, &[]), vec![RuleProblem::NoDestinations]);
    }

    #[test]
    fn a_blank_name_is_refused() {
        let mut s = spec(path(1, "Pictures"));
        s.name = "   ".to_owned();
        assert_eq!(
            check(&s, &[path(2, "Backups")]),
            vec![RuleProblem::EmptyName]
        );
    }

    #[test]
    fn duplicate_destinations_are_reported_once() {
        let s = spec(path(1, "Pictures"));
        let problems = check(&s, &[path(2, "Backups"), path(2, "Backups")]);
        assert_eq!(
            problems,
            vec![RuleProblem::DuplicateDestination {
                destination_index: 1
            }]
        );
    }

    #[test]
    fn every_problem_is_reported_not_just_the_first() {
        // The editor shows them together; revealing them one save at a time
        // is a miserable way to fill in a form.
        let mut s = spec(path(1, "Pictures"));
        s.name = String::new();
        let problems = check(&s, &[path(1, "Pictures/Inside")]);
        assert!(problems.contains(&RuleProblem::EmptyName));
        assert!(problems.contains(&RuleProblem::DestinationInsideSource {
            destination_index: 0
        }));
    }

    #[test]
    fn problems_point_at_the_destination_that_caused_them() {
        let s = spec(path(1, "Pictures"));
        let problems = check(&s, &[path(2, "Backups"), path(1, "Pictures/Inside")]);
        assert_eq!(
            problems.first().and_then(RuleProblem::destination_index),
            Some(1)
        );
        assert_eq!(RuleProblem::EmptyName.destination_index(), None);
    }

    fn plan_with(deletions: usize, destination_files: u64) -> Plan {
        Plan {
            deletions: (0..deletions)
                .map(|n| crate::engine::planner::PlannedDeletion {
                    relative: PathBuf::from(format!("file-{n}.raw")),
                    bytes: 1,
                })
                .collect(),
            destination_files,
            ..Plan::default()
        }
    }

    #[test]
    fn an_unattended_run_that_would_empty_the_backup_is_refused() {
        // The case this exists for: a source that mounted empty, or was
        // renamed. The mirror is working exactly as configured, and the
        // result is a backup with nothing in it.
        let refusal = deletion_refusal(&plan_with(500, 500), false).expect("a refusal");
        assert_eq!(refusal.deletions, 500);
        assert_eq!(refusal.destination_files, 500);
        assert!(format!("{refusal}").contains("Nothing was changed"));
    }

    #[test]
    fn an_ordinary_tidy_up_is_not_refused() {
        // Ten files out of a thousand is a Tuesday.
        assert_eq!(deletion_refusal(&plan_with(10, 1000), false), None);
    }

    #[test]
    fn a_small_folder_losing_most_of_itself_is_not_refused() {
        // Three files out of four is three quarters, and it is also nothing
        // — which is why a share alone cannot be the whole rule.
        assert_eq!(deletion_refusal(&plan_with(3, 4), false), None);
    }

    #[test]
    fn the_threshold_is_a_quarter_and_the_boundary_is_not_refused() {
        // Exactly a quarter passes; one more file does not.
        assert_eq!(deletion_refusal(&plan_with(25, 100), false), None);
        assert!(deletion_refusal(&plan_with(26, 100), false).is_some());
    }

    #[test]
    fn a_run_somebody_asked_for_is_never_refused() {
        // Backup Now shows the deletions first and waits. Refusing after
        // that would be arguing with a decision already made, with the
        // numbers on screen.
        assert_eq!(deletion_refusal(&plan_with(500, 500), true), None);
    }

    #[test]
    fn a_snapshot_is_unaffected_because_it_never_deletes() {
        assert_eq!(deletion_refusal(&Plan::default(), false), None);
    }
}

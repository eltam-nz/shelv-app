//! Aggregates assembled for the UI.
//!
//! The rule table needs a rule plus its tags, destinations, last result and
//! whether each destination is reachable right now. Assembling that here
//! rather than in the Tauri layer keeps the shell thin and lets the whole
//! thing be tested without a window.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Destination, Rule, Run, StoredVolume, Tag};
use crate::platform::{DriveType, PlatformFs};
use crate::store::Store;
use crate::Result;

/// Why a destination cannot be written to, or that it can.
///
/// The mock-up shows only AVAILABLE and UNAVAILABLE. That is not enough: a
/// drive that is unplugged and a drive that is plugged in but *refused* need
/// different words, because only one of them is fixed by plugging it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Attached, permitted, ready to write.
    Available,
    /// Not currently attached. Plugging the drive in resolves this.
    Disconnected,
    /// Attached, but Shelv refuses this kind of volume — a network share,
    /// optical media, or one it could not classify (`docs/PLAN.md` §4.2).
    Refused,
    /// Attached at the recorded mount point, but the volume identity there
    /// does not match. Almost always a different disk that inherited the
    /// drive letter (`docs/PLAN.md` §1.1a). Never written to.
    IdentityMismatch,
}

impl Availability {
    /// Whether a backup may be written here.
    #[must_use]
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// A volume and whether it can be used right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct VolumeStatus {
    /// What the database knows about this volume.
    pub volume: StoredVolume,
    /// Whether it can be written to, and if not, why.
    pub availability: Availability,
    /// Where it is mounted right now, if it is attached.
    pub mount_point: Option<PathBuf>,
}

/// A destination with its current reachability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct DestinationStatus {
    /// The destination itself.
    pub destination: Destination,
    /// The volume it lives on, and whether that volume is usable.
    pub status: VolumeStatus,
}

/// One row of the rule table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RuleRow {
    /// The rule.
    pub rule: Rule,
    /// Its tags, for the first column and for filtering.
    pub tags: Vec<Tag>,
    /// Where it writes, and whether those places are reachable.
    pub destinations: Vec<DestinationStatus>,
    /// The source volume's status. A rule whose *source* is missing cannot
    /// run either, which the mock-up does not account for.
    pub source: VolumeStatus,
    /// The most recent run, or `None` if it has never run. The table shows
    /// "Never run" for `None`, which is distinct from a failure.
    pub last_run: Option<Run>,
}

impl RuleRow {
    /// Whether the rule could run right now.
    ///
    /// Requires the source and at least one destination to be writable: a rule
    /// with two destinations, one unplugged, can still back up to the other.
    #[must_use]
    pub fn is_runnable(&self) -> bool {
        self.rule.spec.enabled
            && self.source.availability.is_writable()
            && self
                .destinations
                .iter()
                .any(|d| d.status.availability.is_writable())
    }
}

/// Classifies every recorded volume against what is attached right now.
pub fn volume_statuses(store: &Store, fs: &dyn PlatformFs) -> Result<Vec<VolumeStatus>> {
    let attached = fs.volumes()?;
    Ok(store
        .volumes()?
        .into_iter()
        .map(|volume| classify(volume, &attached))
        .collect())
}

fn classify(volume: StoredVolume, attached: &[crate::platform::VolumeInfo]) -> VolumeStatus {
    // Matched on identity, never on mount point. A drive that came back as a
    // different letter is the same volume; a different drive that inherited
    // the letter is not.
    let live = attached.iter().find(|a| a.identity == volume.identity);

    let Some(live) = live else {
        // Nothing attached carries this identity. If something *is* mounted
        // where we last saw it, that is a different disk wearing the same
        // drive letter, which is the case worth naming separately.
        let impostor = volume.last_mount.as_ref().is_some_and(|last| {
            attached
                .iter()
                .any(|a| a.mount_point == *last && a.identity != volume.identity)
        });
        return VolumeStatus {
            availability: if impostor {
                Availability::IdentityMismatch
            } else {
                Availability::Disconnected
            },
            mount_point: None,
            volume,
        };
    };

    let availability = if live.drive_type.is_permitted() {
        Availability::Available
    } else {
        Availability::Refused
    };

    VolumeStatus {
        availability,
        mount_point: Some(live.mount_point.clone()),
        volume,
    }
}

/// Builds every row of the rule table.
pub fn rule_rows(store: &Store, fs: &dyn PlatformFs) -> Result<Vec<RuleRow>> {
    let statuses = volume_statuses(store, fs)?;
    let status_for =
        |id| -> Option<VolumeStatus> { statuses.iter().find(|s| s.volume.id == id).cloned() };

    store
        .rules()?
        .into_iter()
        .map(|rule| {
            let destinations = store
                .destinations(rule.id)?
                .into_iter()
                .map(|destination| {
                    let status = status_for(destination.path.volume)
                        .unwrap_or_else(|| unknown_volume(destination.path.volume));
                    DestinationStatus {
                        destination,
                        status,
                    }
                })
                .collect();

            Ok(RuleRow {
                tags: store.rule_tags(rule.id)?,
                destinations,
                source: status_for(rule.spec.source.volume)
                    .unwrap_or_else(|| unknown_volume(rule.spec.source.volume)),
                last_run: store.last_run(rule.id)?,
                rule,
            })
        })
        .collect()
}

/// Stands in for a volume row that has gone missing from the database.
///
/// Foreign keys make this unreachable in practice. It exists so the table can
/// still render — dropping the row would hide a rule from the user, which is
/// worse than showing it as unusable.
fn unknown_volume(id: crate::model::VolumeId) -> VolumeStatus {
    VolumeStatus {
        volume: StoredVolume {
            id,
            identity: crate::platform::VolumeIdentity {
                kind: crate::platform::VolumeIdentityKind::LinuxFsUuid,
                value: String::new(),
            },
            serial: None,
            label: None,
            filesystem: None,
            drive_type: DriveType::Unknown,
            is_sync_root: false,
            last_seen_at: None,
            last_mount: None,
        },
        availability: Availability::Disconnected,
        mount_point: None,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;
    use crate::platform::{CaseSensitivity, VolumeIdentity, VolumeIdentityKind, VolumeInfo};

    fn identity(value: &str) -> VolumeIdentity {
        VolumeIdentity {
            kind: VolumeIdentityKind::WindowsVolumeGuid,
            value: value.to_owned(),
        }
    }

    fn info(value: &str, mount: &str, drive_type: DriveType) -> VolumeInfo {
        VolumeInfo {
            identity: identity(value),
            mount_point: PathBuf::from(mount),
            serial: None,
            label: Some("Backup".to_owned()),
            filesystem: Some("exFAT".to_owned()),
            drive_type,
            case_sensitivity: CaseSensitivity::Insensitive,
            is_sync_root: false,
        }
    }

    fn stored(id: i64, value: &str, last_mount: Option<&str>) -> StoredVolume {
        StoredVolume {
            id: crate::model::VolumeId(id),
            identity: identity(value),
            serial: None,
            label: Some("Backup".to_owned()),
            filesystem: Some("exFAT".to_owned()),
            drive_type: DriveType::Removable,
            is_sync_root: false,
            last_seen_at: None,
            last_mount: last_mount.map(PathBuf::from),
        }
    }

    #[test]
    fn an_attached_permitted_volume_is_available() {
        let attached = [info("vol-a", "E:\\", DriveType::Removable)];
        let status = classify(stored(1, "vol-a", Some("E:\\")), &attached);
        assert_eq!(status.availability, Availability::Available);
        assert_eq!(status.mount_point, Some(PathBuf::from("E:\\")));
        assert!(status.availability.is_writable());
    }

    #[test]
    fn a_volume_that_moved_letters_is_still_available() {
        // The whole point of identity-based matching: same disk, new letter.
        let attached = [info("vol-a", "F:\\", DriveType::Removable)];
        let status = classify(stored(1, "vol-a", Some("E:\\")), &attached);
        assert_eq!(status.availability, Availability::Available);
        assert_eq!(status.mount_point, Some(PathBuf::from("F:\\")));
    }

    #[test]
    fn a_missing_volume_is_disconnected() {
        let attached = [info("vol-b", "E:\\", DriveType::Removable)];
        let status = classify(stored(1, "vol-a", None), &attached);
        assert_eq!(status.availability, Availability::Disconnected);
        assert_eq!(status.mount_point, None);
    }

    #[test]
    fn a_different_disk_on_the_old_letter_is_an_identity_mismatch() {
        // This is the case that would destroy data if it were treated as
        // Available: E: exists, but it is not the drive the rule was built
        // against (docs/PLAN.md §1.1a).
        let attached = [info("someone-elses-disk", "E:\\", DriveType::Removable)];
        let status = classify(stored(1, "vol-a", Some("E:\\")), &attached);
        assert_eq!(status.availability, Availability::IdentityMismatch);
        assert!(!status.availability.is_writable());
        assert_eq!(status.mount_point, None, "never offer the wrong volume");
    }

    #[test]
    fn an_attached_network_share_is_refused_not_available() {
        let attached = [info("vol-a", "Z:\\", DriveType::Network)];
        let status = classify(stored(1, "vol-a", None), &attached);
        assert_eq!(status.availability, Availability::Refused);
        assert!(!status.availability.is_writable());
    }

    #[test]
    fn only_available_counts_as_writable() {
        assert!(Availability::Available.is_writable());
        assert!(!Availability::Disconnected.is_writable());
        assert!(!Availability::Refused.is_writable());
        assert!(!Availability::IdentityMismatch.is_writable());
    }
}

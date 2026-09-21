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
    /// Attached, but the system exposes nothing that identifies this volume
    /// across reconnections. Shelv cannot tell it apart from a different
    /// drive appearing at the same place, so it will not write to it.
    Unverifiable,
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

/// One row of the drives table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct DriveRow {
    /// The drive and whether it can be written to right now.
    pub status: VolumeStatus,
    /// How many rules use it, as a source or as a destination.
    ///
    /// Shown so the user can see what forgetting a drive would break before
    /// they try, rather than being told only when it is refused.
    #[ts(type = "number")]
    pub rule_count: u32,
}

/// Every drive Shelv has recorded, with its current status and use.
///
/// Recorded drives only. A drive enters the database by the user picking a
/// folder on it through the operating system's dialog (`docs/PLAN.md` §4.2),
/// and enumerating attached-but-unknown drives here would hand the frontend
/// a list of the user's hardware for no gain — the picker is itself the OS's
/// list of what is attached, so that is where a new drive is added from.
pub fn drive_rows(store: &Store, fs: &dyn PlatformFs) -> Result<Vec<DriveRow>> {
    volume_statuses(store, fs)?
        .into_iter()
        .map(|status| {
            let rule_count = store.volume_references(status.volume.id)?;
            Ok(DriveRow {
                status,
                rule_count: u32::try_from(rule_count).unwrap_or(u32::MAX),
            })
        })
        .collect()
}

/// Where Shelv keeps what it remembers.
///
/// Both paths are derived from the user's local application data folder, not
/// from where the executable sits — which is why replacing the binary keeps
/// every rule, drive and nickname. That is the intent: an update must not
/// lose someone's backup configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct DataLocations {
    /// The `SQLite` database: rules, destinations, tags, drives and history.
    pub database: PathBuf,
    /// The webview's profile directory, which holds the window's own
    /// preferences. Filled in by the shell, which is what decides where the
    /// webview keeps it.
    pub webview_profile: PathBuf,
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

    // Order matters: an unverifiable identity is reported as such even on a
    // permitted device type, because "Refused" would suggest the wrong fix.
    let availability = if !live.identity.kind.is_stable() {
        Availability::Unverifiable
    } else if live.drive_type.is_permitted() {
        Availability::Available
    } else {
        Availability::Refused
    };

    // What the drive says about itself *now* beats what was recorded when a
    // folder was last picked on it. A drive renamed in Explorer, reformatted
    // to a different filesystem, or moved from a USB caddy into a bay is
    // still the same volume — the identity says so — but the stored label,
    // filesystem and drive type are all now wrong, and the label is the name
    // the table puts in front of the user. Rows would keep showing the old
    // name until the user happened to pick another folder on that drive.
    //
    // Persisting the change is a separate matter, handled by the caller
    // (`refresh_stored_volumes`), because classification must stay a pure
    // function the UI can call as often as it likes.
    VolumeStatus {
        availability,
        mount_point: Some(live.mount_point.clone()),
        volume: StoredVolume {
            label: live.label.clone(),
            filesystem: live.filesystem.clone(),
            drive_type: live.drive_type,
            serial: live.serial.clone(),
            is_sync_root: live.is_sync_root,
            last_mount: Some(live.mount_point.clone()),
            ..volume
        },
    }
}

/// Writes back any volume whose live details differ from what is recorded.
///
/// Only ever an update. A volume enters the database by being picked
/// (`docs/PLAN.md` §4.2) and nothing here may add one, or merely attaching a
/// drive would register it — which is exactly the thing picking exists to
/// gate.
///
/// Returns how many rows changed, so a caller that runs on a timer can log
/// or skip on zero rather than writing every tick.
pub fn refresh_stored_volumes(store: &Store, fs: &dyn PlatformFs, now: i64) -> Result<usize> {
    let attached = fs.volumes()?;
    let mut changed = 0;

    for recorded in store.volumes()? {
        let Some(live) = attached.iter().find(|a| a.identity == recorded.identity) else {
            continue;
        };
        let differs = recorded.label != live.label
            || recorded.filesystem != live.filesystem
            || recorded.drive_type != live.drive_type
            || recorded.serial != live.serial
            || recorded.is_sync_root != live.is_sync_root
            || recorded.last_mount.as_ref() != Some(&live.mount_point);
        if !differs {
            continue;
        }
        store.refresh_volume(recorded.id, live, now)?;
        changed += 1;
    }

    Ok(changed)
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
            nickname: None,
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
            nickname: None,
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
    fn a_renamed_drive_reports_its_new_name_immediately() {
        // Renaming a drive in Explorer must not leave the table showing the
        // old name until the user happens to pick another folder on it.
        let mut live = info("vol-a", "E:\\", DriveType::Removable);
        live.label = Some("Photos 2026".to_owned());

        let status = classify(stored(1, "vol-a", Some("E:\\")), &[live]);
        assert_eq!(status.volume.label.as_deref(), Some("Photos 2026"));
    }

    #[test]
    fn a_disconnected_drive_keeps_the_last_name_it_was_seen_under() {
        // There is nothing live to read a name from, and "Unnamed" would be
        // a worse answer than the name the user last saw.
        let status = classify(stored(1, "vol-a", Some("E:\\")), &[]);
        assert_eq!(status.availability, Availability::Disconnected);
        assert_eq!(status.volume.label.as_deref(), Some("Backup"));
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
        assert!(!Availability::Unverifiable.is_writable());
    }

    #[test]
    fn an_attached_volume_with_no_stable_identity_is_unverifiable() {
        // Permitted device type, physically present, and still refused: the
        // system offers nothing that would identify it next time.
        let mut live = info("/media/backup", "/media/backup", DriveType::Removable);
        live.identity.kind = VolumeIdentityKind::Unverified;

        let mut record = stored(1, "/media/backup", Some("/media/backup"));
        record.identity.kind = VolumeIdentityKind::Unverified;

        let status = classify(record, &[live]);
        assert_eq!(status.availability, Availability::Unverifiable);
        assert!(!status.availability.is_writable());
    }
}

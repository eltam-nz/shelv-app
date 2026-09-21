//! Noticing that the set of attached drives has changed.
//!
//! The rule table's whole job is to say which backups can run right now, and
//! that answer changes the moment a drive is plugged in or pulled out. A
//! table that only refreshes when the window is reopened is not merely stale,
//! it is misleading: it says "Disconnected" next to a drive the user is
//! looking at.
//!
//! # Why this polls
//!
//! The better primitive on Windows is `WM_DEVICECHANGE` with
//! `DBT_DEVTYP_VOLUME`, which the OS delivers the instant a volume arrives
//! or leaves, with no periodic work at all. It is also Windows-only, needs a
//! window handle and a subclassed window procedure — so `unsafe`, in the
//! shell rather than behind the platform boundary — and cannot be exercised
//! anywhere but on Windows, which makes it exactly the kind of code that is
//! written once and then quietly rots.
//!
//! Polling reaches the same place by a duller route. One enumeration a
//! second is cheap next to what it is guarding, it behaves identically on
//! both target platforms, and it is ordinary testable code. The part worth
//! getting right is that the *poll* and the *notification* are separate:
//! this type compares each poll against the last and reports a change only
//! when there is one, so the frontend re-renders on device events rather
//! than sixty times a minute.
//!
//! Replacing the timer with a real device-event source later means calling
//! [`VolumeWatch::poll`] from that event instead of from a tick. Nothing
//! else has to move.

use std::time::Duration;

use crate::platform::PlatformFs;
use crate::store::Store;
use crate::view::{refresh_stored_volumes, volume_statuses, VolumeStatus};
use crate::Result;

/// How often to look, when driven by a timer.
///
/// A second is under the threshold at which the table feels stale after
/// plugging a drive in, and well above the cost of enumerating volumes.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What a poll found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poll {
    /// Every recorded volume and its current availability.
    pub statuses: Vec<VolumeStatus>,
    /// Whether this differs from the previous poll. `true` on the first one.
    pub changed: bool,
}

/// Remembers the last observed volume statuses so repeats can be ignored.
#[derive(Debug, Default)]
pub struct VolumeWatch {
    last: Option<Vec<VolumeStatus>>,
}

impl VolumeWatch {
    /// A watch that has not looked yet, so its first poll always reports a
    /// change.
    #[must_use]
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Re-reads the attached volumes and says whether anything moved.
    ///
    /// Also writes back a drive that has been renamed, so the new name
    /// outlives the session. That write is skipped entirely when nothing
    /// differs, which is every tick but the interesting ones.
    ///
    /// The comparison covers availability, mount point and the volume's own
    /// details, so a rename counts as a change just as a disconnection does
    /// — the name is on screen, so a stale one is a wrong answer.
    pub fn poll(&mut self, store: &Store, fs: &dyn PlatformFs, now: i64) -> Result<Poll> {
        refresh_stored_volumes(store, fs, now)?;
        let statuses = volume_statuses(store, fs)?;
        let changed = self.last.as_ref() != Some(&statuses);
        if changed {
            self.last = Some(statuses.clone());
        }
        Ok(Poll { statuses, changed })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;
    use crate::platform::{
        CaseSensitivity, DriveType, SpaceInfo, VolumeIdentity, VolumeIdentityKind, VolumeInfo,
    };
    use crate::volumes::resolve_picked_folder;
    use crate::CoreError;

    /// A platform whose attached volumes the test can change between polls.
    struct SwappableFs {
        volumes: Mutex<Vec<VolumeInfo>>,
    }

    impl SwappableFs {
        fn set(&self, volumes: Vec<VolumeInfo>) {
            *self.volumes.lock().unwrap() = volumes;
        }
    }

    impl PlatformFs for SwappableFs {
        fn volumes(&self) -> Result<Vec<VolumeInfo>> {
            Ok(self.volumes.lock().unwrap().clone())
        }

        fn volume_of(&self, path: &Path) -> Result<VolumeInfo> {
            self.volumes
                .lock()
                .unwrap()
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

    fn volume(mount: &str, label: &str) -> VolumeInfo {
        VolumeInfo {
            identity: VolumeIdentity {
                kind: VolumeIdentityKind::LinuxFsUuid,
                value: format!("uuid{mount}"),
            },
            mount_point: PathBuf::from(mount),
            serial: None,
            label: Some(label.to_owned()),
            filesystem: Some("ext4".to_owned()),
            drive_type: DriveType::Removable,
            case_sensitivity: CaseSensitivity::Sensitive,
            is_sync_root: false,
        }
    }

    fn fixture() -> (Store, SwappableFs) {
        let store = Store::open_in_memory().unwrap();
        let fs = SwappableFs {
            volumes: Mutex::new(vec![volume("/media/backup", "Backup")]),
        };
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 0).unwrap();
        (store, fs)
    }

    #[test]
    fn the_first_poll_always_reports_a_change() {
        // Nothing has been rendered yet, so there is no "same as before".
        let (store, fs) = fixture();
        let mut watch = VolumeWatch::new();
        assert!(watch.poll(&store, &fs, 1).unwrap().changed);
    }

    #[test]
    fn an_unchanged_poll_reports_no_change() {
        // The case that happens ~86,000 times a day. If it reported a change
        // the UI would rebuild every second for nothing.
        let (store, fs) = fixture();
        let mut watch = VolumeWatch::new();
        watch.poll(&store, &fs, 1).unwrap();
        assert!(!watch.poll(&store, &fs, 2).unwrap().changed);
        assert!(!watch.poll(&store, &fs, 3).unwrap().changed);
    }

    #[test]
    fn unplugging_and_replugging_a_drive_each_report_a_change() {
        let (store, fs) = fixture();
        let mut watch = VolumeWatch::new();
        watch.poll(&store, &fs, 1).unwrap();

        fs.set(Vec::new());
        let gone = watch.poll(&store, &fs, 2).unwrap();
        assert!(gone.changed);
        assert_eq!(
            gone.statuses.first().unwrap().availability,
            crate::view::Availability::Disconnected
        );

        fs.set(vec![volume("/media/backup", "Backup")]);
        let back = watch.poll(&store, &fs, 3).unwrap();
        assert!(back.changed);
        assert_eq!(
            back.statuses.first().unwrap().availability,
            crate::view::Availability::Available
        );
    }

    #[test]
    fn renaming_a_drive_counts_as_a_change() {
        // The drive never left, so availability is identical. The name is on
        // screen though, so this still has to reach the UI.
        let (store, fs) = fixture();
        let mut watch = VolumeWatch::new();
        watch.poll(&store, &fs, 1).unwrap();

        let mut renamed = volume("/media/backup", "Backup");
        renamed.label = Some("Archive".to_owned());
        fs.set(vec![renamed]);

        let poll = watch.poll(&store, &fs, 2).unwrap();
        assert!(poll.changed);
        assert_eq!(
            poll.statuses.first().unwrap().volume.label.as_deref(),
            Some("Archive")
        );
        // And it settles again rather than reporting a change forever.
        assert!(!watch.poll(&store, &fs, 3).unwrap().changed);
    }

    #[test]
    fn a_drive_nobody_picked_a_folder_on_is_not_reported_at_all() {
        // Polling must not become a back door that enrols drives, since
        // picking a folder is the consent step (`docs/PLAN.md` §4.2).
        let (store, fs) = fixture();
        let mut watch = VolumeWatch::new();
        watch.poll(&store, &fs, 1).unwrap();

        fs.set(vec![
            volume("/media/backup", "Backup"),
            volume("/media/stranger", "Not Mine"),
        ]);

        let poll = watch.poll(&store, &fs, 2).unwrap();
        assert!(!poll.changed, "a strange drive is not a change to report");
        assert_eq!(poll.statuses.len(), 1);
        assert_eq!(store.volumes().unwrap().len(), 1);
    }
}

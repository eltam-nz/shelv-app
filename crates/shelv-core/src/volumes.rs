//! Turning a folder the user picked into something a rule can refer to.
//!
//! This is the only way a location enters Shelv. The native folder dialog is
//! the operating system's own consent step, and everything downstream deals
//! in `(volume id, relative path)` rather than absolute paths — so a rule can
//! only ever point somewhere the user personally chose (`docs/PLAN.md` §4.2).
//!
//! It is also where volumes get recorded at all: until a folder is picked on
//! a drive, Shelv has no row for it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::{VolumeId, VolumePath};
use crate::platform::PlatformFs;
use crate::store::Store;
use crate::Result;

/// A folder the user picked, resolved against its volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PickedFolder {
    /// Where this is, in the coordinates a rule stores: a volume and a path
    /// relative to its mount point.
    pub path: VolumePath,
    /// The volume's label, for display.
    pub volume_label: Option<String>,
    /// The volume serial, where the platform reports one.
    ///
    /// Display only, and only as a fallback: it is what lets two unlabelled
    /// drives that have taken turns in the same port be told apart in the
    /// editor, which would otherwise call both of them `E:\`.
    pub volume_serial: Option<String>,
    /// Where the volume is mounted right now.
    ///
    /// **Display only.** It is returned so the editor can show the user the
    /// full path they just chose; nothing accepts it back as input.
    pub mount_point: PathBuf,
    /// The full path as the user would recognise it, for a tooltip.
    pub display_path: String,
}

/// Records the volume a picked folder sits on and resolves the folder
/// against it.
///
/// Refuses a volume Shelv will not use, rather than recording it and failing
/// later when a backup is attempted. The volume row is only written once the
/// volume has passed, so a refusal leaves no trace in the database.
pub fn resolve_picked_folder(
    store: &Store,
    fs: &dyn PlatformFs,
    absolute: &Path,
    now: i64,
) -> Result<PickedFolder> {
    let info = fs.volume_of(absolute)?;

    if !info.drive_type.is_permitted() {
        return Err(CoreError::Refused(format!(
            "Shelv does not back up to this kind of volume ({:?}). Network shares, optical \
             media and drives it cannot classify are excluded.",
            info.drive_type
        )));
    }

    if !info.identity.kind.is_stable() {
        return Err(CoreError::Refused(
            "the system reports nothing that identifies this drive across reconnections, so \
             Shelv cannot tell it apart from a different drive plugged into the same place"
                .to_owned(),
        ));
    }

    // The mount point is a prefix of the picked path by construction, since
    // `volume_of` derived it from that path. Guard anyway rather than assume.
    let relative = absolute
        .strip_prefix(&info.mount_point)
        .map_err(|_| {
            CoreError::Invalid(format!(
                "{} is not inside {}",
                absolute.display(),
                info.mount_point.display()
            ))
        })?
        .to_path_buf();

    let volume = store.upsert_volume(&info, Some(now))?;

    Ok(PickedFolder {
        path: VolumePath { volume, relative },
        volume_label: info.label.clone(),
        volume_serial: info.serial.clone(),
        mount_point: info.mount_point.clone(),
        display_path: absolute.display().to_string(),
    })
}

/// Rebuilds the absolute path for a stored location, if its volume is
/// attached right now.
///
/// The engine will use this to turn a rule back into somewhere it can write.
/// It returns `None` rather than guessing when the volume is absent, and
/// refuses outright when the volume at that identity is not the one recorded
/// — that is the wrong-drive case (`docs/PLAN.md` §1.1a).
pub fn resolve_stored_path(
    store: &Store,
    fs: &dyn PlatformFs,
    path: &VolumePath,
) -> Result<Option<PathBuf>> {
    let recorded = store.volume(path.volume)?;

    if !recorded.identity.kind.is_stable() {
        return Err(CoreError::Refused(format!(
            "volume {} has no identity that can be verified",
            path.volume
        )));
    }

    let Some(live) = fs
        .volumes()?
        .into_iter()
        .find(|v| v.identity == recorded.identity)
    else {
        return Ok(None);
    };

    if !live.is_usable() {
        return Err(CoreError::Refused(format!(
            "volume {} is attached but cannot be used",
            path.volume
        )));
    }

    Ok(Some(live.mount_point.join(&path.relative)))
}

/// The volume id for a stored path, for callers that only need the id.
#[must_use]
pub const fn volume_of(path: &VolumePath) -> VolumeId {
    path.volume
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;
    use crate::platform::{
        CaseSensitivity, DriveType, SpaceInfo, VolumeIdentity, VolumeIdentityKind, VolumeInfo,
    };

    /// A platform whose volume set is fixed by the test.
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

    fn volume(mount: &str, kind: VolumeIdentityKind, drive_type: DriveType) -> VolumeInfo {
        VolumeInfo {
            identity: VolumeIdentity {
                kind,
                value: format!("id-for-{mount}"),
            },
            mount_point: PathBuf::from(mount),
            serial: None,
            label: Some("Backup Drive".to_owned()),
            filesystem: Some("exFAT".to_owned()),
            drive_type,
            case_sensitivity: CaseSensitivity::Insensitive,
            is_sync_root: false,
        }
    }

    fn fs_with(volumes: Vec<VolumeInfo>) -> FakeFs {
        FakeFs { volumes }
    }

    #[test]
    fn picking_a_folder_registers_its_volume_and_returns_relative_coordinates() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        )]);

        assert_eq!(store.volumes().unwrap().len(), 0, "nothing recorded yet");

        let picked =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/Photos/2026"), 100)
                .unwrap();

        // The path a rule stores is relative, never absolute.
        assert_eq!(picked.path.relative, PathBuf::from("Photos/2026"));
        assert_eq!(picked.volume_label.as_deref(), Some("Backup Drive"));
        assert_eq!(picked.display_path, "/media/backup/Photos/2026");

        // Picking is what puts a volume in the database.
        let recorded = store.volumes().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded.first().map(|v| v.id), Some(picked.path.volume));
    }

    #[test]
    fn picking_the_volume_root_yields_an_empty_relative_path() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        )]);

        let picked = resolve_picked_folder(&store, &fs, Path::new("/media/backup"), 100).unwrap();
        assert_eq!(picked.path.relative, PathBuf::from(""));
    }

    #[test]
    fn a_network_share_is_refused_and_leaves_no_row() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/mnt/nas",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Network,
        )]);

        let error = resolve_picked_folder(&store, &fs, Path::new("/mnt/nas/Backups"), 100)
            .expect_err("a network share must be refused");
        assert!(matches!(error, CoreError::Refused(_)), "{error:?}");

        // A refusal must not pollute the database with a volume that can
        // never be used.
        assert_eq!(store.volumes().unwrap().len(), 0);
    }

    #[test]
    fn an_unidentifiable_drive_is_refused_even_though_its_type_is_fine() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/mystery",
            VolumeIdentityKind::Unverified,
            DriveType::Removable,
        )]);

        let error = resolve_picked_folder(&store, &fs, Path::new("/media/mystery/Backups"), 100)
            .expect_err("an unverifiable identity must be refused");
        assert!(matches!(error, CoreError::Refused(_)), "{error:?}");
        assert_eq!(store.volumes().unwrap().len(), 0);
    }

    #[test]
    fn picking_the_same_drive_twice_reuses_its_row() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        )]);

        let first =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/One"), 100).unwrap();
        let second =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/Two"), 200).unwrap();

        assert_eq!(first.path.volume, second.path.volume);
        assert_eq!(store.volumes().unwrap().len(), 1);
    }

    #[test]
    fn the_deepest_mount_wins_for_a_nested_volume() {
        // /media/backup mounted inside /, so a path under it belongs to the
        // inner volume, not the outer one.
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![
            volume("/", VolumeIdentityKind::LinuxFsUuid, DriveType::Fixed),
            volume(
                "/media/backup",
                VolumeIdentityKind::LinuxFsUuid,
                DriveType::Removable,
            ),
        ]);

        let picked =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/Photos"), 100).unwrap();
        assert_eq!(picked.mount_point, PathBuf::from("/media/backup"));
        assert_eq!(picked.path.relative, PathBuf::from("Photos"));
    }

    #[test]
    fn a_stored_path_resolves_back_to_wherever_the_drive_is_now() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        )]);
        let picked =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/Photos"), 100).unwrap();

        // Same drive, mounted somewhere else. Matching is on identity, so it
        // still resolves — to the new location.
        let mut moved = volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        );
        moved.mount_point = PathBuf::from("/run/media/user/backup");
        let fs_moved = fs_with(vec![moved]);

        let resolved = resolve_stored_path(&store, &fs_moved, &picked.path).unwrap();
        assert_eq!(
            resolved,
            Some(PathBuf::from("/run/media/user/backup/Photos"))
        );
    }

    #[test]
    fn a_stored_path_on_an_absent_drive_resolves_to_nothing() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        )]);
        let picked =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/Photos"), 100).unwrap();

        // Nothing attached. It must not fall back to the last known mount
        // point, which may now belong to a different disk.
        let empty = fs_with(Vec::new());
        assert_eq!(
            resolve_stored_path(&store, &empty, &picked.path).unwrap(),
            None
        );
    }

    #[test]
    fn a_different_drive_at_the_same_mount_point_does_not_resolve() {
        let store = Store::open_in_memory().unwrap();
        let fs = fs_with(vec![volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        )]);
        let picked =
            resolve_picked_folder(&store, &fs, Path::new("/media/backup/Photos"), 100).unwrap();

        // Same location, different disk. Resolving to it would write the
        // backup onto a stranger's drive.
        let mut impostor = volume(
            "/media/backup",
            VolumeIdentityKind::LinuxFsUuid,
            DriveType::Removable,
        );
        impostor.identity.value = "some-other-disk".to_owned();

        assert_eq!(
            resolve_stored_path(&store, &fs_with(vec![impostor]), &picked.path).unwrap(),
            None,
            "a different disk at the same mount point must not resolve"
        );
    }
}

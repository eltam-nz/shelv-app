//! The platform boundary.
//!
//! Everything OS-specific lives behind [`PlatformFs`] and [`CloudPlaceholders`].
//! The engine depends on the traits, never on a concrete implementation, so
//! porting to Linux means writing one module rather than unpicking the core
//! (`docs/PLAN.md` §2.6).
//!
//! Two rules hold this boundary in place, and CI checks both:
//!
//! * the `windows` crate is imported only under `platform::windows`;
//! * `unsafe` appears only under `platform`, and only to call into the OS.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

#[cfg(windows)]
pub mod windows;

#[cfg(unix)]
pub mod unix;

/// How a volume is identified across reconnections.
///
/// Drive letters and mount points are **not** identities: Windows reassigns
/// letters from a registry mapping, so a rule keyed on `E:` can end up writing
/// to a different disk (`docs/PLAN.md` §1.1a). The value is opaque to the
/// engine and to the database; only the platform layer interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum VolumeIdentityKind {
    /// A Windows volume GUID path, `\\?\Volume{...}\`.
    WindowsVolumeGuid,
    /// A Linux filesystem UUID, as published under `/dev/disk/by-uuid`.
    LinuxFsUuid,
    /// **No stable identity could be determined.** The value is the mount
    /// point, which is exactly what this type exists to avoid relying on: it
    /// changes between sessions, so a reconnected drive looks like a new
    /// volume and a different drive at the same location looks like this one.
    ///
    /// Shelv will not write to such a volume. It is still enumerated, so the
    /// UI can explain why rather than silently omitting it.
    Unverified,
}

impl VolumeIdentityKind {
    /// Whether an identity of this kind survives the volume being
    /// disconnected and reattached somewhere else.
    ///
    /// Everything that decides whether a backup may be written must go
    /// through this rather than testing for a specific variant, so that a
    /// future identity scheme is refused until it is explicitly trusted.
    #[must_use]
    pub const fn is_stable(self) -> bool {
        match self {
            Self::WindowsVolumeGuid | Self::LinuxFsUuid => true,
            Self::Unverified => false,
        }
    }
}

/// A stable, platform-specific volume identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct VolumeIdentity {
    /// Which scheme `value` is written in.
    pub kind: VolumeIdentityKind,
    /// The opaque identity itself.
    pub value: String,
}

/// What kind of device backs a volume.
///
/// Shelv accepts only [`Fixed`](DriveType::Fixed) and
/// [`Removable`](DriveType::Removable); the rest are refused so that a rule
/// cannot be pointed at a network share (`docs/PLAN.md` §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum DriveType {
    /// An internal disk.
    Fixed,
    /// Media that comes out of the device: a USB flash stick, a memory card,
    /// a card reader.
    ///
    /// **This does not mean "external".** Windows' `GetDriveType` reports
    /// `DRIVE_REMOVABLE` for removable *media*, so a USB hard disk or SSD in
    /// an enclosure — fixed media inside a device you unplug — comes back as
    /// [`Fixed`](Self::Fixed). Anything that wants to know whether a drive
    /// comes and goes must not ask this; Shelv assumes every destination
    /// does (see `DriveLocation` in the UI).
    Removable,
    /// SMB, NFS, sshfs or another remote filesystem. Always refused.
    Network,
    /// Optical media. Refused.
    Optical,
    /// A RAM disk. Refused, since its contents do not survive a reboot.
    RamDisk,
    /// Could not be classified. Refused, because refusing is the safe default.
    Unknown,
}

impl DriveType {
    /// Whether Shelv will read from or write to a volume of this kind.
    ///
    /// The default is refusal: anything unrecognised is treated as unusable
    /// rather than assumed local.
    #[must_use]
    pub const fn is_permitted(self) -> bool {
        matches!(self, Self::Fixed | Self::Removable)
    }
}

/// Whether a filesystem distinguishes `Photo.RAW` from `photo.raw`.
///
/// NTFS is insensitive, ext4 is sensitive, and exFAT differs by host. The diff
/// logic must not assume either, so it compares through this rather than
/// through `Path`'s own `==`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum CaseSensitivity {
    /// `a.txt` and `A.txt` are different files.
    Sensitive,
    /// `a.txt` and `A.txt` are the same file.
    Insensitive,
}

impl CaseSensitivity {
    /// Folds a path into a key that compares and hashes correctly for this
    /// filesystem.
    ///
    /// Used to index the source tree when diffing against the destination. On
    /// a case-insensitive volume two paths differing only in case must collide,
    /// or the engine would copy the same file twice and, in mirror mode,
    /// consider one of them extraneous and delete it.
    #[must_use]
    pub fn fold(self, path: &Path) -> OsString {
        match self {
            Self::Sensitive => path.as_os_str().to_os_string(),
            // `to_string_lossy` is banned crate-wide for paths that get
            // reopened, but a fold is only ever a comparison key: it is never
            // turned back into a path, and a lossy byte folds consistently on
            // both sides of the diff.
            #[allow(clippy::disallowed_methods)]
            Self::Insensitive => OsString::from(path.to_string_lossy().to_lowercase()),
        }
    }

    /// Whether two paths refer to the same file on this filesystem.
    #[must_use]
    pub fn same_path(self, a: &Path, b: &Path) -> bool {
        self.fold(a) == self.fold(b)
    }
}

/// A volume as the platform currently sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct VolumeInfo {
    /// Stable identity, used to recognise this volume on reconnection.
    pub identity: VolumeIdentity,
    /// Where the volume is mounted right now. Display and resolution only;
    /// never persisted as the thing that identifies the volume.
    pub mount_point: PathBuf,
    /// Volume serial, where the platform exposes one. Changes on reformat, so
    /// it corroborates `identity` rather than replacing it.
    pub serial: Option<String>,
    /// Human-readable label, for the UI.
    pub label: Option<String>,
    /// Filesystem name, e.g. `NTFS`, `exFAT`, `ext4`. Drives the mtime
    /// tolerance: FAT32 stores timestamps to two seconds (`docs/PLAN.md` §1.1f).
    pub filesystem: Option<String>,
    /// What backs the volume.
    pub drive_type: DriveType,
    /// How this filesystem compares names.
    pub case_sensitivity: CaseSensitivity,
    /// Whether this volume is, or contains, a cloud sync root. Shelv treats
    /// such paths as read-only sources.
    pub is_sync_root: bool,
}

impl VolumeInfo {
    /// Whether this volume may be read from or written to.
    ///
    /// Requires both a permitted device type and an identity that will still
    /// mean the same thing next time the drive appears.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.drive_type.is_permitted() && self.identity.kind.is_stable()
    }

    /// The mtime difference below which two files count as unchanged.
    ///
    /// FAT32 records modification times in two-second units, so an exact
    /// comparison against an NTFS source marks every file as changed and
    /// re-copies the whole tree on every run.
    #[must_use]
    pub fn mtime_tolerance(&self) -> std::time::Duration {
        let coarse = self
            .filesystem
            .as_deref()
            .is_some_and(|fs| fs.eq_ignore_ascii_case("FAT32") || fs.eq_ignore_ascii_case("vfat"));
        std::time::Duration::from_secs(if coarse { 2 } else { 0 })
    }
}

/// Free and total bytes on a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct SpaceInfo {
    /// Bytes currently available to this user.
    #[ts(type = "number")]
    pub available: u64,
    /// Total capacity.
    #[ts(type = "number")]
    pub total: u64,
}

/// Filesystem and volume services the engine needs from the host OS.
pub trait PlatformFs: Send + Sync {
    /// Every volume currently attached, including ones Shelv will refuse.
    ///
    /// Refused volumes are returned rather than filtered so the UI can explain
    /// *why* a destination is unusable instead of silently omitting it.
    fn volumes(&self) -> Result<Vec<VolumeInfo>>;

    /// The volume containing `path`.
    fn volume_of(&self, path: &Path) -> Result<VolumeInfo>;

    /// Free and total space on the volume containing `path`.
    fn space(&self, path: &Path) -> Result<SpaceInfo>;

    /// Rewrites a path into the form the OS accepts for deep trees.
    ///
    /// On Windows this applies the `\\?\` prefix, without which paths beyond
    /// 260 characters fail — routine in a Lightroom tree. Elsewhere it is the
    /// identity function.
    fn long_path(&self, path: &Path) -> PathBuf;

    /// Where Shelv keeps its database and logs.
    fn data_dir(&self) -> Result<PathBuf>;

    /// Sends a file to the recycle bin or trash.
    ///
    /// Mirror mode uses this instead of unlinking, so that a deletion caused by
    /// a mis-configured rule is recoverable (`docs/PLAN.md` §4, T4).
    fn trash(&self, path: &Path) -> Result<()>;
}

/// Whether a file's content is actually present on local disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderState {
    /// An ordinary file. Its bytes are on disk.
    Full,
    /// A cloud placeholder with no local content. Reading it downloads it.
    Dehydrated,
    /// A cloud placeholder that is partly local.
    PartiallyHydrated,
    /// A cloud file whose content is fully local.
    Hydrated,
}

impl PlaceholderState {
    /// Whether reading this file would pull bytes over the network.
    #[must_use]
    pub const fn needs_hydration(self) -> bool {
        matches!(self, Self::Dehydrated | Self::PartiallyHydrated)
    }
}

/// Whether the user has asked for a cloud file to be kept on local disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum PinState {
    /// "Always keep on this device". Shelv must never clear this.
    Pinned,
    /// Free to be evicted by the sync engine.
    Unpinned,
    /// Neither pinned nor unpinned; the sync engine decides.
    Unspecified,
}

/// Cloud-placeholder operations, as used by the `OneDrive` hydration path.
///
/// Implemented for real only on Windows. Other platforms get a no-op
/// implementation reporting every file as [`PlaceholderState::Full`], so the
/// engine needs no `cfg` of its own (`docs/PLAN.md` §1.2).
pub trait CloudPlaceholders: Send + Sync {
    /// Whether `path` is a placeholder, and whether its content is local.
    ///
    /// Must not open the file: opening a dehydrated placeholder is exactly what
    /// triggers the download this call exists to predict.
    fn placeholder_state(&self, path: &Path) -> Result<PlaceholderState>;

    /// The file's current pin state.
    ///
    /// Recorded before hydration so that a file the user had pinned is never
    /// released afterwards (`docs/PLAN.md` §4, T5).
    fn pin_state(&self, path: &Path) -> Result<PinState>;

    /// Requests a change of pin state.
    ///
    /// Setting [`PinState::Unpinned`] asks the sync engine to reclaim the local
    /// copy. It is a request, not a guarantee, and it completes asynchronously:
    /// callers must not assume the space is free when this returns.
    fn set_pin_state(&self, path: &Path, state: PinState) -> Result<()>;

    /// Whether this platform has cloud placeholders at all.
    fn is_supported(&self) -> bool;
}

/// The [`PlatformFs`] implementation for the host this build targets.
#[must_use]
pub fn host_fs() -> Box<dyn PlatformFs> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsFs::new())
    }
    #[cfg(all(unix, not(windows)))]
    {
        Box::new(unix::UnixFs::new())
    }
}

/// The [`CloudPlaceholders`] implementation for the host this build targets.
#[must_use]
pub fn host_cloud() -> Box<dyn CloudPlaceholders> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsCloud::new())
    }
    #[cfg(all(unix, not(windows)))]
    {
        Box::new(NoCloud)
    }
}

/// Stand-in for platforms without cloud placeholders.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCloud;

impl CloudPlaceholders for NoCloud {
    fn placeholder_state(&self, _path: &Path) -> Result<PlaceholderState> {
        Ok(PlaceholderState::Full)
    }

    fn pin_state(&self, _path: &Path) -> Result<PinState> {
        Ok(PinState::Unspecified)
    }

    fn set_pin_state(&self, _path: &Path, _state: PinState) -> Result<()> {
        Ok(())
    }

    fn is_supported(&self) -> bool {
        false
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

    #[test]
    fn only_local_drive_types_are_permitted() {
        assert!(DriveType::Fixed.is_permitted());
        assert!(DriveType::Removable.is_permitted());
        // Refusing network destinations is the whole point of the gate.
        assert!(!DriveType::Network.is_permitted());
        assert!(!DriveType::Optical.is_permitted());
        assert!(!DriveType::RamDisk.is_permitted());
        // Unclassifiable volumes are refused, not assumed safe.
        assert!(!DriveType::Unknown.is_permitted());
    }

    #[test]
    fn case_folding_matches_the_filesystem() {
        let a = Path::new("Photos/DSC_0001.NEF");
        let b = Path::new("photos/dsc_0001.nef");

        assert!(!CaseSensitivity::Sensitive.same_path(a, b));
        assert!(CaseSensitivity::Insensitive.same_path(a, b));
        // Genuinely different names must not collide either way.
        let c = Path::new("photos/dsc_0002.nef");
        assert!(!CaseSensitivity::Insensitive.same_path(a, c));
    }

    #[test]
    fn fat32_gets_a_two_second_mtime_tolerance() {
        let volume = |fs: &str| VolumeInfo {
            identity: VolumeIdentity {
                kind: VolumeIdentityKind::LinuxFsUuid,
                value: "x".into(),
            },
            mount_point: PathBuf::from("/mnt/x"),
            serial: None,
            label: None,
            filesystem: Some(fs.to_owned()),
            drive_type: DriveType::Removable,
            case_sensitivity: CaseSensitivity::Insensitive,
            is_sync_root: false,
        };

        // Without this, every file on a FAT32 backup drive looks modified and
        // the whole tree is re-copied on every run.
        assert_eq!(volume("FAT32").mtime_tolerance().as_secs(), 2);
        assert_eq!(volume("vfat").mtime_tolerance().as_secs(), 2);
        assert_eq!(volume("NTFS").mtime_tolerance().as_secs(), 0);
        assert_eq!(volume("exFAT").mtime_tolerance().as_secs(), 0);
    }

    #[test]
    fn an_unverifiable_identity_is_never_stable() {
        assert!(VolumeIdentityKind::WindowsVolumeGuid.is_stable());
        assert!(VolumeIdentityKind::LinuxFsUuid.is_stable());
        assert!(!VolumeIdentityKind::Unverified.is_stable());
    }

    #[test]
    fn a_permitted_drive_with_no_stable_identity_is_not_usable() {
        // The case that would otherwise slip through: the device type is
        // fine, so every drive-type check passes, but the identity is a
        // mount point and cannot be verified on reconnection.
        let volume = |kind| VolumeInfo {
            identity: VolumeIdentity {
                kind,
                value: "/media/backup".into(),
            },
            mount_point: PathBuf::from("/media/backup"),
            serial: None,
            label: None,
            filesystem: Some("exFAT".to_owned()),
            drive_type: DriveType::Removable,
            case_sensitivity: CaseSensitivity::Insensitive,
            is_sync_root: false,
        };

        assert!(volume(VolumeIdentityKind::LinuxFsUuid).is_usable());
        assert!(!volume(VolumeIdentityKind::Unverified).is_usable());
    }

    #[test]
    fn placeholder_states_that_require_a_download() {
        assert!(PlaceholderState::Dehydrated.needs_hydration());
        assert!(PlaceholderState::PartiallyHydrated.needs_hydration());
        assert!(!PlaceholderState::Full.needs_hydration());
        assert!(!PlaceholderState::Hydrated.needs_hydration());
    }

    #[test]
    fn no_cloud_reports_plain_files() {
        let cloud = NoCloud;
        assert!(!cloud.is_supported());
        assert_eq!(
            cloud.placeholder_state(Path::new("/tmp/x")).unwrap(),
            PlaceholderState::Full
        );
    }
}

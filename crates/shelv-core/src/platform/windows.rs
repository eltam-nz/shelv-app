//! Windows implementation of the platform boundary.
//!
//! This is the only module that imports the `windows` crate, and the only one
//! besides [`super::unix`] that uses `unsafe`. CI enforces both.

// Every `unsafe` block below is a Win32 call, each with its own SAFETY note.
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Storage::FileSystem::{
    FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetFileAttributesW, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW,
    GetVolumePathNameW, GetVolumePathNamesForVolumeNameW, SetFileAttributesW,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_UNPINNED, FILE_FLAGS_AND_ATTRIBUTES,
    INVALID_FILE_ATTRIBUTES,
};
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
};
use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use crate::error::CoreError;
use crate::platform::{
    CaseSensitivity, CloudPlaceholders, DriveType, LocalTime, PinState, PlaceholderState,
    PlatformFs, SpaceInfo, VolumeIdentity, VolumeIdentityKind, VolumeInfo,
};
use crate::Result;

/// The prefix that lifts the 260-character `MAX_PATH` limit.
const LONG_PATH_PREFIX: &str = r"\\?\";

/// Encodes a path as a NUL-terminated UTF-16 buffer for the Win32 `W` APIs.
fn wide(path: &OsStr) -> Vec<u16> {
    path.encode_wide().chain(std::iter::once(0)).collect()
}

/// Decodes a NUL-terminated UTF-16 buffer returned by Win32.
fn from_wide(buf: &[u16]) -> OsString {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    // Indexing is bounded by `position`, which cannot exceed `buf.len()`.
    OsString::from_wide(buf.get(..end).unwrap_or(&[]))
}

/// The longest a volume GUID path can be, plus room for a NUL.
const VOLUME_NAME_LEN: usize = 64;

/// Whether an `OsStr` begins with an ASCII prefix, compared in UTF-16 so that
/// no lossy conversion is needed.
fn starts_with_wide(text: &OsStr, prefix: &str) -> bool {
    let expected: Vec<u16> = prefix.encode_utf16().collect();
    text.encode_wide()
        .take(expected.len())
        .eq(expected.iter().copied())
}

/// [`PlatformFs`] for Windows.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsFs;

impl WindowsFs {
    /// The implementation for the running system.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// The mount root containing `path`, e.g. `C:\` or a mounted-folder root.
    fn volume_root(path: &Path) -> Result<PathBuf> {
        let input = wide(path.as_os_str());
        let mut buf = [0u16; 261];
        // SAFETY: `input` is NUL-terminated and outlives the call; `buf` is a
        // writable slice whose length is passed as the capacity.
        unsafe {
            GetVolumePathNameW(PCWSTR(input.as_ptr()), &mut buf).map_err(|e| {
                CoreError::Io(format!(
                    "could not resolve the volume for {}: {e}",
                    path.display()
                ))
            })?;
        }
        Ok(PathBuf::from(from_wide(&buf)))
    }

    /// Reads the label, filesystem name and serial for a mount root.
    fn volume_information(root: &Path) -> (Option<String>, Option<String>, Option<String>) {
        let input = wide(root.as_os_str());
        let mut label = [0u16; 261];
        let mut fs_name = [0u16; 261];
        let mut serial: u32 = 0;

        // SAFETY: all three buffers are writable and correctly sized, and
        // `input` is a NUL-terminated string that outlives the call. Failure
        // is reported by the return value and leaves the buffers untouched.
        let ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(input.as_ptr()),
                Some(&mut label),
                Some(&raw mut serial),
                None,
                None,
                Some(&mut fs_name),
            )
        }
        .is_ok();

        if !ok {
            // A removable bay with no media in it fails here. That is normal,
            // not an error: the volume simply has nothing to report.
            return (None, None, None);
        }

        let to_opt = |s: OsString| {
            let s = s.to_string_lossy().into_owned();
            (!s.is_empty()).then_some(s)
        };
        (
            to_opt(from_wide(&label)),
            to_opt(from_wide(&fs_name)),
            Some(format!("{serial:08X}")),
        )
    }

    /// Classifies a mount root via `GetDriveTypeW`.
    ///
    /// This is the first of the two layers that keep Shelv off network shares
    /// (`docs/PLAN.md` §4.2).
    fn drive_type(root: &Path) -> DriveType {
        let input = wide(root.as_os_str());
        // SAFETY: `input` is a NUL-terminated string that outlives the call.
        let raw = unsafe { GetDriveTypeW(PCWSTR(input.as_ptr())) };
        // These are plain `u32` constants, not a newtype. Written as bare
        // `match` arms they would be irrefutable bindings that match anything,
        // so every drive — a network share included — would come back Fixed
        // and the gate in `DriveType::is_permitted` would pass everything.
        // Compare explicitly.
        if raw == DRIVE_FIXED {
            DriveType::Fixed
        } else if raw == DRIVE_REMOVABLE {
            DriveType::Removable
        } else if raw == DRIVE_REMOTE {
            DriveType::Network
        } else if raw == DRIVE_CDROM {
            DriveType::Optical
        } else if raw == DRIVE_RAMDISK {
            DriveType::RamDisk
        } else {
            DriveType::Unknown
        }
    }

    /// The volume GUID path for a mount root, e.g. `\\?\Volume{...}\`.
    ///
    /// This is the identity everything else hangs off: it survives the drive
    /// being unplugged and reattached under a different letter, which a
    /// letter itself does not (`docs/PLAN.md` §1.1a).
    fn volume_guid(root: &Path) -> Option<String> {
        let input = wide(root.as_os_str());
        let mut buf = [0u16; VOLUME_NAME_LEN];
        // SAFETY: `input` is NUL-terminated and outlives the call; `buf` is a
        // writable slice whose length is passed as the capacity.
        let ok =
            unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(input.as_ptr()), &mut buf).is_ok() };
        if !ok {
            // An empty removable bay, or a root that is not a mount point.
            return None;
        }
        let name = from_wide(&buf);
        // A GUID path is ASCII by construction.
        #[allow(clippy::disallowed_methods)]
        let text = Path::new(&name).to_string_lossy().into_owned();
        (!text.is_empty()).then_some(text)
    }

    /// Builds a [`VolumeInfo`] for a mount root.
    fn describe(root: &Path) -> VolumeInfo {
        let (label, filesystem, serial) = Self::volume_information(root);
        let identity = Self::volume_guid(root).map_or_else(
            || VolumeIdentity {
                // No GUID means nothing stable to key on. Recording the mount
                // point as if it were an identity is what would let a backup
                // land on the wrong disk, so it is marked unverifiable and
                // `VolumeInfo::is_usable` refuses it.
                kind: VolumeIdentityKind::Unverified,
                #[allow(clippy::disallowed_methods)]
                value: root.to_string_lossy().into_owned(),
            },
            |guid| VolumeIdentity {
                kind: VolumeIdentityKind::WindowsVolumeGuid,
                value: guid,
            },
        );

        VolumeInfo {
            identity,
            mount_point: root.to_path_buf(),
            serial,
            label,
            filesystem,
            drive_type: Self::drive_type(root),
            // NTFS, exFAT and FAT32 all ignore case as mounted by Windows.
            // Per-directory case sensitivity exists but is off by default.
            case_sensitivity: CaseSensitivity::Insensitive,
            is_sync_root: false,
        }
    }

    /// Every volume GUID path the system knows about, mounted or not.
    fn enumerate_volume_guids() -> Result<Vec<String>> {
        let mut buf = [0u16; VOLUME_NAME_LEN];
        // SAFETY: `buf` is a writable slice whose length is passed as the
        // capacity. The returned handle is closed on every path out below.
        let handle = unsafe { FindFirstVolumeW(&mut buf) }
            .map_err(|e| CoreError::Io(format!("could not enumerate volumes: {e}")))?;

        let mut out = Vec::new();
        loop {
            #[allow(clippy::disallowed_methods)]
            let name = Path::new(&from_wide(&buf)).to_string_lossy().into_owned();
            if !name.is_empty() {
                out.push(name);
            }
            // SAFETY: `handle` came from FindFirstVolumeW and has not been
            // closed; `buf` is writable and its length is passed along.
            if unsafe { FindNextVolumeW(handle, &mut buf) }.is_err() {
                // ERROR_NO_MORE_FILES is the normal end of the walk.
                break;
            }
        }

        // SAFETY: `handle` is still open and is not used again afterwards.
        let _ = unsafe { FindVolumeClose(handle) };
        Ok(out)
    }

    /// The paths a volume is mounted at. Empty for an unmounted volume.
    fn mount_points_for(guid: &str) -> Vec<PathBuf> {
        let input = wide(OsStr::new(guid));
        let mut needed: u32 = 0;

        // Ask for the required size first: a volume can be mounted at several
        // paths, and a fixed buffer would silently truncate the list.
        // SAFETY: `input` is NUL-terminated and outlives the call; passing no
        // buffer is how the API reports the size it needs.
        let _ = unsafe {
            GetVolumePathNamesForVolumeNameW(PCWSTR(input.as_ptr()), None, &raw mut needed)
        };
        if needed == 0 {
            return Vec::new();
        }

        let mut buf = vec![0u16; needed as usize];
        // SAFETY: `buf` is sized to what the call above asked for, and
        // `input` still outlives this call.
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                PCWSTR(input.as_ptr()),
                Some(&mut buf),
                &raw mut needed,
            )
        }
        .is_ok();
        if !ok {
            return Vec::new();
        }

        // The result is a sequence of NUL-terminated strings ending in a
        // second NUL.
        buf.split(|c| *c == 0)
            .filter(|part| !part.is_empty())
            .map(|part| PathBuf::from(OsString::from_wide(part)))
            .collect()
    }
}

impl PlatformFs for WindowsFs {
    fn volumes(&self) -> Result<Vec<VolumeInfo>> {
        // Walks real volumes rather than drive letters A–Z. A letter is a
        // display convenience that the system reassigns; the GUID is not.
        // This also finds volumes mounted into a folder, which have no letter
        // at all and which a letter scan would miss entirely.
        let mut out = Vec::new();
        for guid in Self::enumerate_volume_guids()? {
            for mount in Self::mount_points_for(&guid) {
                let (label, filesystem, serial) = Self::volume_information(&mount);
                out.push(VolumeInfo {
                    identity: VolumeIdentity {
                        kind: VolumeIdentityKind::WindowsVolumeGuid,
                        value: guid.clone(),
                    },
                    drive_type: Self::drive_type(&mount),
                    mount_point: mount,
                    serial,
                    label,
                    filesystem,
                    case_sensitivity: CaseSensitivity::Insensitive,
                    is_sync_root: false,
                });
            }
            // A volume with no mount point cannot be backed up to, so it is
            // not reported: there is no path for a rule to name.
        }
        Ok(out)
    }

    fn volume_of(&self, path: &Path) -> Result<VolumeInfo> {
        Ok(Self::describe(&Self::volume_root(path)?))
    }

    fn space(&self, path: &Path) -> Result<SpaceInfo> {
        let root = Self::volume_root(path)?;
        let input = wide(root.as_os_str());
        let mut available: u64 = 0;
        let mut total: u64 = 0;

        // SAFETY: `input` is NUL-terminated and outlives the call; both output
        // pointers address live `u64`s that the API only writes on success.
        unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(input.as_ptr()),
                Some(&raw mut available),
                Some(&raw mut total),
                None,
            )
            .map_err(|e| {
                CoreError::Io(format!(
                    "could not read free space on {}: {e}",
                    root.display()
                ))
            })?;
        }

        Ok(SpaceInfo { available, total })
    }

    fn long_path(&self, path: &Path) -> PathBuf {
        let text = path.as_os_str();
        if text.encode_wide().count() < 240 {
            return path.to_path_buf();
        }
        // Already prefixed, or a UNC path, which needs the \\?\UNC\ form
        // rather than a bare prefix and is out of scope: Shelv refuses network
        // destinations anyway (`docs/PLAN.md` §4.2).
        if starts_with_wide(text, LONG_PATH_PREFIX) || starts_with_wide(text, r"\\") {
            return path.to_path_buf();
        }
        if !path.is_absolute() {
            // The prefix disables all path normalisation, so a relative path
            // would be interpreted literally and resolve to the wrong place.
            return path.to_path_buf();
        }
        // Concatenated as `OsString`, not through a lossy `String`: Windows
        // paths are UTF-16 and an unpaired surrogate would not survive the
        // round trip, leaving us opening a different file than intended.
        let mut out = OsString::with_capacity(text.len() + LONG_PATH_PREFIX.len());
        out.push(LONG_PATH_PREFIX);
        out.push(text);
        PathBuf::from(out)
    }

    fn data_dir(&self) -> Result<PathBuf> {
        // Resolves via SHGetKnownFolderPath, which is correct even when
        // %LOCALAPPDATA% is unset or redirected.
        dirs::data_local_dir()
            .map(|d| d.join("Shelv"))
            .ok_or_else(|| CoreError::Io("could not determine %LOCALAPPDATA%".into()))
    }

    fn trash(&self, _path: &Path) -> Result<()> {
        // Needs IFileOperation with FOF_ALLOWUNDO. Lands with mirror deletions
        // in M1; refusing is correct until then, because the alternative is an
        // unrecoverable delete (`docs/PLAN.md` §4, T4).
        Err(CoreError::Invalid(
            "moving files to the recycle bin is not implemented yet".into(),
        ))
    }
}

/// [`CloudPlaceholders`] backed by the Windows cloud-files attributes.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsCloud;

impl WindowsCloud {
    /// The implementation for the running system.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn attributes(path: &Path) -> Result<FILE_FLAGS_AND_ATTRIBUTES> {
        let input = wide(path.as_os_str());
        // SAFETY: `input` is a NUL-terminated string that outlives the call.
        // This reads directory-entry metadata only; it does not open the file,
        // which is what matters here — opening a dehydrated placeholder is
        // exactly what triggers the download (`docs/PLAN.md` §1.2).
        let raw = unsafe { GetFileAttributesW(PCWSTR(input.as_ptr())) };
        if raw == INVALID_FILE_ATTRIBUTES {
            return Err(CoreError::Io(format!(
                "could not read attributes of {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }
        Ok(FILE_FLAGS_AND_ATTRIBUTES(raw))
    }
}

impl CloudPlaceholders for WindowsCloud {
    fn placeholder_state(&self, path: &Path) -> Result<PlaceholderState> {
        let attrs = Self::attributes(path)?;
        let has = |flag: FILE_FLAGS_AND_ATTRIBUTES| attrs.0 & flag.0 != 0;

        // RECALL_ON_DATA_ACCESS marks a Files On-Demand placeholder whose
        // content is not local. OFFLINE and RECALL_ON_OPEN cover the older
        // remote-storage conventions.
        if has(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) {
            return Ok(PlaceholderState::Dehydrated);
        }
        if has(FILE_ATTRIBUTE_OFFLINE) || has(FILE_ATTRIBUTE_RECALL_ON_OPEN) {
            return Ok(PlaceholderState::Dehydrated);
        }
        // Pinned or unpinned without a recall flag means a cloud file whose
        // content is already on disk.
        if has(FILE_ATTRIBUTE_PINNED) || has(FILE_ATTRIBUTE_UNPINNED) {
            return Ok(PlaceholderState::Hydrated);
        }
        Ok(PlaceholderState::Full)
    }

    fn pin_state(&self, path: &Path) -> Result<PinState> {
        let attrs = Self::attributes(path)?;
        if attrs.0 & FILE_ATTRIBUTE_PINNED.0 != 0 {
            Ok(PinState::Pinned)
        } else if attrs.0 & FILE_ATTRIBUTE_UNPINNED.0 != 0 {
            Ok(PinState::Unpinned)
        } else {
            Ok(PinState::Unspecified)
        }
    }

    fn set_pin_state(&self, path: &Path, state: PinState) -> Result<()> {
        let current = Self::attributes(path)?.0;
        let cleared = current & !FILE_ATTRIBUTE_PINNED.0 & !FILE_ATTRIBUTE_UNPINNED.0;
        let next = match state {
            PinState::Pinned => cleared | FILE_ATTRIBUTE_PINNED.0,
            // The documented way to ask for dehydration is to clear the pinned
            // attribute and set the unpinned one. It is a request: the sync
            // engine reclaims the space asynchronously, and may not at all.
            PinState::Unpinned => cleared | FILE_ATTRIBUTE_UNPINNED.0,
            PinState::Unspecified => cleared,
        };

        let input = wide(path.as_os_str());
        // SAFETY: `input` is a NUL-terminated string that outlives the call,
        // and `next` is derived from the attributes Windows just returned.
        unsafe {
            SetFileAttributesW(PCWSTR(input.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(next)).map_err(
                |e| {
                    CoreError::Io(format!(
                        "could not set pin state on {}: {e}",
                        path.display()
                    ))
                },
            )?;
        }
        Ok(())
    }

    fn is_supported(&self) -> bool {
        true
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
    fn wide_round_trips() {
        let original = OsStr::new(r"C:\Users\Test\Photos");
        let encoded = wide(original);
        assert_eq!(encoded.last(), Some(&0), "must be NUL terminated");
        assert_eq!(from_wide(&encoded), original);
    }

    #[test]
    fn short_paths_are_left_alone() {
        let fs = WindowsFs::new();
        let p = Path::new(r"C:\Photos\2026\a.nef");
        assert_eq!(fs.long_path(p), p);
    }

    #[test]
    fn deep_paths_get_the_long_prefix() {
        let fs = WindowsFs::new();
        let deep = PathBuf::from(format!(r"C:\{}", "segment\\".repeat(40)));
        let out = fs.long_path(&deep);
        assert!(
            starts_with_wide(out.as_os_str(), LONG_PATH_PREFIX),
            "a path past MAX_PATH must be prefixed or the copy fails"
        );
    }

    #[test]
    fn unc_and_relative_paths_are_never_prefixed() {
        let fs = WindowsFs::new();
        // Prefixing a UNC path needs the \\?\UNC\ form, not a bare prefix.
        let unc = PathBuf::from(format!(r"\\server\share\{}", "segment\\".repeat(40)));
        assert!(!starts_with_wide(
            fs.long_path(&unc).as_os_str(),
            LONG_PATH_PREFIX
        ));

        // The prefix disables normalisation, so a relative path would resolve
        // somewhere else entirely.
        let relative = PathBuf::from("segment\\".repeat(40));
        assert!(!starts_with_wide(
            fs.long_path(&relative).as_os_str(),
            LONG_PATH_PREFIX
        ));
    }

    #[test]
    fn already_prefixed_paths_are_not_double_prefixed() {
        let fs = WindowsFs::new();
        let already = PathBuf::from(format!(r"\\?\C:\{}", "segment\\".repeat(40)));
        assert_eq!(fs.long_path(&already), already);
    }

    #[test]
    fn system_volume_is_classified_as_fixed() {
        assert_eq!(WindowsFs::drive_type(Path::new(r"C:\")), DriveType::Fixed);
    }

    #[test]
    fn enumeration_finds_the_system_volume() {
        let volumes = WindowsFs::new().volumes().unwrap();
        assert!(volumes.iter().any(|v| v.mount_point == Path::new(r"C:\")));
    }

    #[test]
    fn space_reports_a_real_volume() {
        let space = WindowsFs::new().space(Path::new(r"C:\")).unwrap();
        assert!(space.total > 0);
        assert!(space.available <= space.total);
    }

    #[test]
    fn an_ordinary_file_is_not_a_placeholder() {
        let cloud = WindowsCloud::new();
        let temp = std::env::temp_dir().join("shelv-placeholder-test.txt");
        std::fs::write(&temp, b"x").unwrap();
        assert_eq!(
            cloud.placeholder_state(&temp).unwrap(),
            PlaceholderState::Full
        );
        assert_eq!(cloud.pin_state(&temp).unwrap(), PinState::Unspecified);
        std::fs::remove_file(&temp).ok();
    }
}

/// 1601-01-01 to 1970-01-01, in the 100-nanosecond units Windows counts in.
const UNIX_EPOCH_IN_TICKS: i64 = 116_444_736_000_000_000;

/// 100-nanosecond units per second.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// The local clock, as Windows reports it.
#[derive(Debug, Clone, Copy)]
pub struct WindowsLocalTime;

impl LocalTime for WindowsLocalTime {
    /// Asks Windows what the instant looks like on the local clock, and
    /// takes the difference.
    ///
    /// `SystemTimeToTzSpecificLocalTime` with no zone means the machine's
    /// current one, and it applies the daylight-saving rule **in force at
    /// that instant** rather than the one in force today. That is the whole
    /// reason this takes an instant instead of returning a constant: a run
    /// in July compared against a run in December must not be a day out
    /// because the offset changed in between.
    ///
    /// Every failure path returns `0` rather than propagating. The caller
    /// is deciding whether it is a new day yet, and a backup tool that
    /// refuses to run because a time zone could not be read would be
    /// choosing the worse of two harms.
    fn utc_offset_seconds(&self, unix_seconds: i64) -> i32 {
        let Some(ticks) = unix_seconds
            .checked_mul(TICKS_PER_SECOND)
            .and_then(|t| t.checked_add(UNIX_EPOCH_IN_TICKS))
        else {
            return 0;
        };
        let Some(utc) = to_filetime(ticks) else {
            return 0;
        };

        let mut broken_down = SYSTEMTIME::default();
        let mut local = SYSTEMTIME::default();
        let mut local_time = FILETIME::default();

        // SAFETY: three Win32 conversions, each writing into a local this
        // function owns and reading one it has already initialised. None
        // retains a pointer past the call, and each result is checked
        // before the next uses its output.
        unsafe {
            if FileTimeToSystemTime(&raw const utc, &raw mut broken_down).is_err() {
                return 0;
            }
            if SystemTimeToTzSpecificLocalTime(None, &raw const broken_down, &raw mut local)
                .is_err()
            {
                return 0;
            }
            if SystemTimeToFileTime(&raw const local, &raw mut local_time).is_err() {
                return 0;
            }
        }

        let difference = from_filetime(local_time).saturating_sub(ticks) / TICKS_PER_SECOND;
        i32::try_from(difference).unwrap_or(0)
    }
}

/// Splits a tick count into the two halves `FILETIME` carries.
fn to_filetime(ticks: i64) -> Option<FILETIME> {
    let ticks = u64::try_from(ticks).ok()?;
    Some(FILETIME {
        dwLowDateTime: u32::try_from(ticks & 0xFFFF_FFFF).ok()?,
        dwHighDateTime: u32::try_from(ticks >> 32).ok()?,
    })
}

/// Rejoins them.
fn from_filetime(time: FILETIME) -> i64 {
    let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    i64::try_from(ticks).unwrap_or(0)
}

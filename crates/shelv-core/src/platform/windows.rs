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
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetFileAttributesW, GetVolumeInformationW,
    GetVolumePathNameW, SetFileAttributesW, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED,
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_UNPINNED,
    FILE_FLAGS_AND_ATTRIBUTES, INVALID_FILE_ATTRIBUTES,
};
use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use crate::error::CoreError;
use crate::platform::{
    CaseSensitivity, CloudPlaceholders, DriveType, PinState, PlaceholderState, PlatformFs,
    SpaceInfo, VolumeIdentity, VolumeIdentityKind, VolumeInfo,
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

    /// Builds a [`VolumeInfo`] for a mount root.
    fn describe(root: &Path) -> VolumeInfo {
        let (label, filesystem, serial) = Self::volume_information(root);
        VolumeInfo {
            identity: VolumeIdentity {
                kind: VolumeIdentityKind::WindowsVolumeGuid,
                // Enumerating true volume GUID paths needs
                // FindFirstVolume/GetVolumePathNamesForVolumeName, which lands
                // with volume tracking in M1. Until then the mount root stands
                // in, and `volumes::` refuses to write to a volume whose
                // identity it cannot verify. A mount root is ASCII ("C:\\") or
                // a GUID path, so the lossy conversion cannot alter it.
                #[allow(clippy::disallowed_methods)]
                value: root.to_string_lossy().into_owned(),
            },
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
}

impl PlatformFs for WindowsFs {
    fn volumes(&self) -> Result<Vec<VolumeInfo>> {
        // Drive letters are a display convenience, never an identity; see
        // `VolumeIdentityKind`. Full volume enumeration arrives in M1.
        let mut out = Vec::new();
        for letter in b'A'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", char::from(letter)));
            let drive_type = Self::drive_type(&root);
            if drive_type == DriveType::Unknown {
                continue;
            }
            out.push(Self::describe(&root));
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

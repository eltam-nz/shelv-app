//! Unix implementation of the platform boundary.
//!
//! Linux is not a v1 target (`docs/PLAN.md` §2.6), but this is a working
//! implementation rather than a stub for two reasons: a stub cannot be tested,
//! and CI runs the core's tests on Linux specifically so the portability of the
//! boundary is proven on every commit instead of asserted.

// The only `unsafe` here is the `statvfs` call below. Confined to the platform
// layer by design; CI checks that no other module relaxes this.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use crate::platform::{
    CaseSensitivity, DriveType, PlatformFs, SpaceInfo, VolumeIdentity, VolumeIdentityKind,
    VolumeInfo,
};
use crate::Result;

/// Filesystems backed by another machine. Always refused as sources or
/// destinations (`docs/PLAN.md` §4.2).
const NETWORK_FS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "afs",
    "ceph",
    "glusterfs",
    "fuse.sshfs",
    "fuse.davfs",
    "davfs",
    "ftpfs",
    "9p",
];

/// Kernel bookkeeping mounts, not storage. Skipped entirely.
const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "autofs",
    "binfmt_misc",
    "efivarfs",
    "rpc_pipefs",
    "nsfs",
    "overlay",
];

/// Filesystems that ignore case, mirroring their behaviour on Windows.
const CASE_INSENSITIVE_FS: &[&str] = &["vfat", "fat", "fat32", "exfat", "ntfs", "ntfs3", "msdos"];

/// A parsed line of `/proc/mounts`.
#[derive(Debug, Clone)]
#[allow(
    clippy::struct_field_names,
    reason = "these are the field names /proc/mounts uses"
)]
struct Mount {
    device: String,
    mount_point: PathBuf,
    fstype: String,
}

/// [`PlatformFs`] for Linux and other Unixes.
#[derive(Debug, Clone, Default)]
pub struct UnixFs {
    /// Overrides `/proc/mounts`, so the parsing and classification logic can be
    /// tested against fixtures instead of whatever the test machine happens to
    /// have mounted.
    mounts_source: Option<String>,
}

impl UnixFs {
    /// The implementation for the running system.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mounts_source: None,
        }
    }

    /// An implementation reading mounts from `contents` in `/proc/mounts`
    /// format, for tests.
    #[must_use]
    pub fn with_mounts(contents: impl Into<String>) -> Self {
        Self {
            mounts_source: Some(contents.into()),
        }
    }

    fn read_mounts(&self) -> Result<Vec<Mount>> {
        let raw = match &self.mounts_source {
            Some(s) => s.clone(),
            None => std::fs::read_to_string("/proc/mounts")?,
        };
        Ok(parse_mounts(&raw))
    }

    /// Maps canonical device paths to filesystem UUIDs, by resolving the
    /// symlinks under `/dev/disk/by-uuid`.
    fn uuid_map() -> HashMap<PathBuf, String> {
        let mut map = HashMap::new();
        let Ok(entries) = std::fs::read_dir("/dev/disk/by-uuid") else {
            return map;
        };
        for entry in entries.flatten() {
            let Ok(target) = std::fs::canonicalize(entry.path()) else {
                continue;
            };
            let uuid = entry.file_name().to_string_lossy().into_owned();
            map.insert(target, uuid);
        }
        map
    }

    /// Reads `/sys/block/<dev>/removable` to tell an external disk from an
    /// internal one.
    fn is_removable(device: &str) -> bool {
        let Some(name) = Path::new(device).file_name() else {
            return false;
        };
        let name = name.to_string_lossy();
        // Partitions (sda1) inherit removability from their parent disk (sda).
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
        let candidates = [
            format!("/sys/block/{name}/removable"),
            format!("/sys/block/{base}/removable"),
        ];
        candidates
            .iter()
            .any(|p| std::fs::read_to_string(p).is_ok_and(|v| v.trim() == "1"))
    }

    fn classify(mount: &Mount) -> DriveType {
        let fs = mount.fstype.as_str();
        if NETWORK_FS.contains(&fs) || fs.starts_with("fuse.") && fs.contains("ssh") {
            return DriveType::Network;
        }
        if fs == "iso9660" || fs == "udf" {
            return DriveType::Optical;
        }
        if fs == "tmpfs" || fs == "ramfs" {
            return DriveType::RamDisk;
        }
        if !mount.device.starts_with("/dev/") {
            return DriveType::Unknown;
        }
        if Self::is_removable(&mount.device) {
            DriveType::Removable
        } else {
            DriveType::Fixed
        }
    }

    fn to_volume(mount: &Mount, uuids: &HashMap<PathBuf, String>) -> VolumeInfo {
        let canonical = std::fs::canonicalize(&mount.device).unwrap_or_else(|_| {
            // A device we cannot canonicalise still deserves an entry, so the
            // UI can show it as unusable rather than omitting it silently.
            PathBuf::from(&mount.device)
        });
        let uuid = uuids.get(&canonical).cloned();

        let case_sensitivity = if CASE_INSENSITIVE_FS
            .iter()
            .any(|f| mount.fstype.eq_ignore_ascii_case(f))
        {
            CaseSensitivity::Insensitive
        } else {
            CaseSensitivity::Sensitive
        };

        // Without a UUID the mount point is the only handle available, and it
        // is *not* stable across reconnection — exactly what identity
        // matching exists to avoid. Mark it unverifiable so
        // `VolumeInfo::is_usable` refuses it, rather than passing a mount
        // point off as an identity.
        #[allow(
            clippy::disallowed_methods,
            reason = "an identity is a display string, never reopened as a path"
        )]
        let identity = uuid.map_or_else(
            || VolumeIdentity {
                kind: VolumeIdentityKind::Unverified,
                value: mount.mount_point.to_string_lossy().into_owned(),
            },
            |value| VolumeIdentity {
                kind: VolumeIdentityKind::LinuxFsUuid,
                value,
            },
        );

        VolumeInfo {
            identity,
            mount_point: mount.mount_point.clone(),
            serial: None,
            label: None,
            filesystem: Some(mount.fstype.clone()),
            drive_type: Self::classify(mount),
            case_sensitivity,
            is_sync_root: false,
        }
    }
}

impl PlatformFs for UnixFs {
    fn volumes(&self) -> Result<Vec<VolumeInfo>> {
        let uuids = Self::uuid_map();
        Ok(self
            .read_mounts()?
            .iter()
            .filter(|m| !PSEUDO_FS.iter().any(|p| m.fstype == *p))
            .map(|m| Self::to_volume(m, &uuids))
            .collect())
    }

    fn volume_of(&self, path: &Path) -> Result<VolumeInfo> {
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        // The containing volume is the mount with the longest matching prefix:
        // /home shadows / for a path under /home.
        self.volumes()?
            .into_iter()
            .filter(|v| target.starts_with(&v.mount_point))
            .max_by_key(|v| v.mount_point.as_os_str().len())
            .ok_or_else(|| {
                CoreError::VolumeUnavailable(format!(
                    "no mounted volume contains {}",
                    target.display()
                ))
            })
    }

    fn space(&self, path: &Path) -> Result<SpaceInfo> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| CoreError::Invalid("path contains an interior NUL byte".into()))?;

        // SAFETY: `c_path` is a valid NUL-terminated C string that outlives the
        // call, and `stat` is a correctly sized, zeroed `statvfs` that the
        // kernel only writes on success.
        let stat = unsafe {
            let mut stat = std::mem::zeroed::<libc::statvfs>();
            if libc::statvfs(c_path.as_ptr(), &raw mut stat) != 0 {
                return Err(CoreError::Io(format!(
                    "statvfs failed for {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                )));
            }
            stat
        };

        // `f_frsize` is u64 on 64-bit Linux and u32 elsewhere, so widen
        // through a cast that is correct either way.
        #[allow(
            clippy::useless_conversion,
            reason = "the source width is target-dependent"
        )]
        let block = u64::from(stat.f_frsize);
        Ok(SpaceInfo {
            available: block.saturating_mul(stat.f_bavail),
            total: block.saturating_mul(stat.f_blocks),
        })
    }

    fn long_path(&self, path: &Path) -> PathBuf {
        // Unix has no 260-character limit and no prefix syntax.
        path.to_path_buf()
    }

    fn data_dir(&self) -> Result<PathBuf> {
        dirs::data_local_dir()
            .map(|d| d.join("shelv"))
            .ok_or_else(|| CoreError::Io("could not determine the user data directory".into()))
    }

    fn trash(&self, _path: &Path) -> Result<()> {
        // Mirror deletions are the one operation that must never be a plain
        // unlink (`docs/PLAN.md` §4, T4). Rather than silently fall back to an
        // unrecoverable delete on a platform that is not a v1 target, refuse.
        Err(CoreError::Invalid(
            "moving files to the trash is not implemented on this platform yet".into(),
        ))
    }
}

/// Parses `/proc/mounts` format, decoding the octal escapes the kernel uses for
/// spaces, tabs, newlines and backslashes in mount points.
fn parse_mounts(contents: &str) -> Vec<Mount> {
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            let mount_point = fields.next()?;
            let fstype = fields.next()?;
            Some(Mount {
                device: unescape(device),
                mount_point: PathBuf::from(unescape(mount_point)),
                fstype: fstype.to_owned(),
            })
        })
        .collect()
}

/// Decodes the `\0NN` octal escapes used in `/proc/mounts`.
fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // A backslash followed by exactly three octal digits is an escape;
        // anything else is a literal backslash in a filename.
        let escape = bytes
            .get(i)
            .is_some_and(|b| *b == b'\\')
            .then(|| s.get(i + 1..i + 4))
            .flatten()
            .filter(|d| d.len() == 3 && d.bytes().all(|b| b.is_ascii_digit() && b < b'8'))
            .and_then(|d| u8::from_str_radix(d, 8).ok());

        match escape {
            Some(byte) => {
                out.push(char::from(byte));
                i += 4;
            }
            None => {
                // Safe to index: `i` is a valid byte offset and we advance by
                // whole characters.
                if let Some(c) = s[i..].chars().next() {
                    out.push(c);
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
proc /proc proc rw,nosuid 0 0
sysfs /sys sysfs rw,nosuid 0 0
/dev/sda2 / ext4 rw,relatime 0 0
/dev/sda1 /boot/efi vfat rw,relatime 0 0
tmpfs /dev/shm tmpfs rw,nosuid 0 0
//nas/photos /mnt/nas cifs rw,relatime 0 0
/dev/sdb1 /media/My\\040Backup\\040Drive exfat rw,relatime 0 0
/dev/sr0 /media/cdrom iso9660 ro,relatime 0 0
";

    fn volumes() -> Vec<VolumeInfo> {
        UnixFs::with_mounts(FIXTURE).volumes().unwrap()
    }

    #[test]
    fn pseudo_filesystems_are_dropped() {
        let mounts: Vec<_> = volumes().iter().map(|v| v.mount_point.clone()).collect();
        assert!(!mounts.contains(&PathBuf::from("/proc")));
        assert!(!mounts.contains(&PathBuf::from("/sys")));
        assert!(mounts.contains(&PathBuf::from("/")));
    }

    #[test]
    fn mount_points_with_spaces_are_decoded() {
        // The kernel writes spaces as \040; a backup drive called
        // "My Backup Drive" is entirely ordinary.
        let v = volumes()
            .into_iter()
            .find(|v| v.filesystem.as_deref() == Some("exfat"))
            .expect("exfat mount present");
        assert_eq!(v.mount_point, PathBuf::from("/media/My Backup Drive"));
    }

    #[test]
    fn network_and_optical_mounts_are_refused() {
        let by_mount = |p: &str| {
            volumes()
                .into_iter()
                .find(|v| v.mount_point == Path::new(p))
                .expect("mount present")
        };

        // The whole point of the gate: a CIFS share can never be a destination.
        assert_eq!(by_mount("/mnt/nas").drive_type, DriveType::Network);
        assert!(!by_mount("/mnt/nas").drive_type.is_permitted());

        assert_eq!(by_mount("/media/cdrom").drive_type, DriveType::Optical);
        assert!(!by_mount("/media/cdrom").drive_type.is_permitted());

        assert_eq!(by_mount("/dev/shm").drive_type, DriveType::RamDisk);
        assert!(!by_mount("/dev/shm").drive_type.is_permitted());
    }

    #[test]
    fn a_mount_with_no_uuid_is_marked_unverifiable() {
        // /dev/disk/by-uuid is absent in containers and on some systems, so
        // this is the common case, not an edge case. Passing the mount point
        // off as an identity would defeat the whole scheme.
        let fs = UnixFs::with_mounts("/dev/sdz9 /media/nouuid ext4 rw 0 0\n");
        let v = fs.volumes().unwrap();
        let first = v.first().expect("one volume");
        assert_eq!(first.identity.kind, VolumeIdentityKind::Unverified);
        assert_eq!(first.identity.value, "/media/nouuid");
        assert!(!first.is_usable(), "an unverifiable volume must be refused");
    }

    #[test]
    fn case_sensitivity_follows_the_filesystem() {
        let by_fs = |f: &str| {
            volumes()
                .into_iter()
                .find(|v| v.filesystem.as_deref() == Some(f))
                .expect("mount present")
        };
        assert_eq!(by_fs("ext4").case_sensitivity, CaseSensitivity::Sensitive);
        assert_eq!(by_fs("vfat").case_sensitivity, CaseSensitivity::Insensitive);
        assert_eq!(
            by_fs("exfat").case_sensitivity,
            CaseSensitivity::Insensitive
        );
    }

    #[test]
    fn fat_mounts_carry_the_coarse_mtime_tolerance() {
        let vfat = volumes()
            .into_iter()
            .find(|v| v.filesystem.as_deref() == Some("vfat"))
            .expect("vfat mount present");
        assert_eq!(vfat.mtime_tolerance().as_secs(), 2);
    }

    #[test]
    fn unescape_handles_literals_and_escapes() {
        assert_eq!(unescape("/simple/path"), "/simple/path");
        assert_eq!(unescape("/a\\040b"), "/a b");
        assert_eq!(unescape("/tab\\011x"), "/tab\tx");
        // \\ is not a three-octal-digit escape, so it stays literal.
        assert_eq!(unescape("/back\\slash"), "/back\\slash");
        // 8 and 9 are not octal digits.
        assert_eq!(unescape("/not\\089"), "/not\\089");
    }

    #[test]
    fn deepest_mount_wins_when_resolving_a_path() {
        let fs = UnixFs::with_mounts(
            "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /home/user/photos ext4 rw 0 0\n",
        );
        let v = fs
            .volume_of(Path::new("/home/user/photos/2026/a.nef"))
            .unwrap();
        assert_eq!(v.mount_point, PathBuf::from("/home/user/photos"));
    }

    #[test]
    fn space_reports_a_real_filesystem() {
        let fs = UnixFs::new();
        let space = fs.space(Path::new("/")).unwrap();
        assert!(space.total > 0, "root filesystem should report a capacity");
        assert!(space.available <= space.total);
    }

    #[test]
    fn long_path_is_identity_on_unix() {
        let fs = UnixFs::new();
        let p = Path::new("/some/very/deep/path");
        assert_eq!(fs.long_path(p), p);
    }

    #[test]
    fn trash_refuses_rather_than_deleting_unrecoverably() {
        let fs = UnixFs::new();
        assert!(fs.trash(Path::new("/tmp/whatever")).is_err());
    }
}

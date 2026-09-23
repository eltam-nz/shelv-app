//! Deciding what a run would do, without doing any of it.
//!
//! The planner walks the source, compares it against what is already at the
//! destination, and produces a [`Plan`]. It opens files for metadata only
//! and writes nothing at all, which is what lets a dry run be exactly the
//! same code path as a real one — a preview that ran different logic from
//! the run it previews would be worse than no preview.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::Layout;
use crate::platform::{CaseSensitivity, PlatformFs};
use crate::Result;

/// How deep the walk will go before giving up.
///
/// Not a limit anyone should reach: it is a backstop against a directory
/// structure that recurses, whether through a filesystem loop or something
/// stranger. Windows' own path limit is far below this even with `\\?\`.
pub const MAX_DEPTH: usize = 64;

/// Why a file is being copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum CopyReason {
    /// The destination has nothing by this name.
    Added,
    /// The sizes differ.
    Resized,
    /// The sizes match but the source is newer by more than the
    /// destination filesystem's timestamp granularity.
    Modified,
}

/// Why an entry was left out of the plan.
///
/// These are not errors — the run continues — but each one means a file the
/// user might have expected to be backed up will not be, so every one is
/// named rather than silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Matched one of the rule's ignore patterns.
    Excluded,
    /// A symbolic link, and the rule does not follow them.
    Symlink,
    /// A link that resolves outside the source tree. Always refused, even
    /// when the rule follows links: otherwise a link placed inside the
    /// source is a way to make Shelv copy anything on the machine
    /// (`docs/PLAN.md` §4, T2).
    EscapesSource,
    /// Deeper than [`MAX_DEPTH`].
    TooDeep,
    /// Could not be read. The run reports `Partial` rather than `Ok`.
    Unreadable,
}

/// One file the run would write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PlannedCopy {
    /// Path relative to the source root. Absolute paths are rebuilt by the
    /// copier from the roots it was given, never carried across the IPC
    /// boundary (`docs/PLAN.md` §4, T1).
    pub relative: PathBuf,
    /// Size in bytes, for progress and for the space check.
    #[ts(type = "number")]
    pub bytes: u64,
    /// Why it is in the plan.
    pub reason: CopyReason,
}

/// One file the run would remove from the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PlannedDeletion {
    /// Path relative to the destination root.
    pub relative: PathBuf,
    /// Size in bytes, so a preview can say how much would go.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// One entry the run would pass over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Skipped {
    /// Path relative to the source root.
    pub relative: PathBuf,
    /// Why it was left out.
    pub reason: SkipReason,
    /// What went wrong, where there is more to say than the reason.
    pub detail: Option<String>,
}

/// Everything a run would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Plan {
    /// Files to write, in walk order.
    pub copies: Vec<PlannedCopy>,
    /// Files to remove. Always empty unless the layout deletes.
    pub deletions: Vec<PlannedDeletion>,
    /// Directories to create, so a source folder that happens to be empty
    /// still appears in the backup.
    pub directories: Vec<PathBuf>,
    /// Entries left out, each with a reason.
    pub skipped: Vec<Skipped>,
    /// Total bytes across [`Self::copies`].
    #[ts(type = "number")]
    pub bytes: u64,
    /// How many files were already at the destination when the plan was
    /// made. Zero for a snapshot, which compares against nothing.
    ///
    /// Carried so that a deletion can be weighed against what is there: ten
    /// files going from a backup of twelve is a different event from ten
    /// going from a backup of ten thousand, and only the first is worth
    /// stopping an unattended run over.
    #[ts(type = "number")]
    pub destination_files: u64,
}

impl Plan {
    /// Whether the run would change anything at the destination.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty() && self.deletions.is_empty() && self.directories.is_empty()
    }

    /// Whether anything could not be read.
    ///
    /// A run that hits one of these finishes `Partial`, never `Ok`: a backup
    /// tool that cannot tell "copied everything" from "copied what it could"
    /// is worse than none (`docs/PLAN.md` §5.1).
    #[must_use]
    pub fn has_unreadable(&self) -> bool {
        self.skipped
            .iter()
            .any(|s| s.reason == SkipReason::Unreadable)
    }
}

/// What the planner needs to know beyond the two roots.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// Mirror compares against the destination and can delete; Snapshot
    /// writes a fresh tree and never does either.
    pub layout: Layout,
    /// Glob patterns to leave out.
    pub excludes: Vec<String>,
    /// Whether to follow symbolic links that stay inside the source tree.
    /// Links leaving it are refused either way.
    pub follow_symlinks: bool,
    /// How the **destination** filesystem compares names. It is the
    /// destination that decides whether two source names collide, and a
    /// collision on a case-insensitive volume would otherwise leave mirror
    /// treating one of the pair as extraneous and deleting it.
    pub case_sensitivity: CaseSensitivity,
    /// How far apart two timestamps can be and still count as equal. FAT32
    /// keeps mtimes to two seconds, so comparing exactly against an NTFS
    /// source marks every file changed and re-copies the tree on every run
    /// (`docs/PLAN.md` §1.1f).
    pub mtime_tolerance: Duration,
}

/// Works out what a run would do.
///
/// `destination` is `None` when there is nothing to compare against — a
/// snapshot, which always writes a fresh tree, or a mirror whose destination
/// folder does not exist yet. In that case everything is an addition and
/// nothing is a deletion.
pub fn plan(
    fs: &dyn PlatformFs,
    source: &Path,
    destination: Option<&Path>,
    options: &PlanOptions,
) -> Result<Plan> {
    let excludes = build_globs(&options.excludes)?;
    let mut plan = Plan::default();

    // The destination side first, so each source entry can be resolved
    // against it in one pass and whatever is left over is extraneous.
    let mut existing = match destination {
        Some(root) if options.layout == Layout::Mirror => index_destination(fs, root, options),
        _ => BTreeMap::new(),
    };

    plan.destination_files = u64::try_from(existing.len()).unwrap_or(u64::MAX);

    walk_source(fs, source, options, &excludes, &mut existing, &mut plan)?;

    // Mirror only. A snapshot never reaches here with a populated map,
    // because it never indexes a destination in the first place — a past
    // snapshot is a record of a moment, and editing it is not a backup.
    if options.layout.deletes_extraneous() {
        plan.deletions = existing
            .into_values()
            .map(|entry| PlannedDeletion {
                relative: entry.relative,
                bytes: entry.bytes,
            })
            .collect();
        plan.deletions.sort_by(|a, b| a.relative.cmp(&b.relative));
    }

    Ok(plan)
}

/// A file already present at the destination.
struct Existing {
    relative: PathBuf,
    bytes: u64,
    modified: Option<SystemTime>,
}

/// Compiles the rule's ignore patterns.
///
/// A pattern that will not parse is refused rather than ignored: silently
/// dropping it would mean backing up something the user asked to leave out,
/// and they would have no way to tell.
fn build_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|e| {
            CoreError::Invalid(format!("ignore pattern {pattern:?} is not valid: {e}"))
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| CoreError::Invalid(format!("could not build the ignore list: {e}")))
}

/// Reads the destination tree into a map keyed by folded relative path.
fn index_destination(
    fs: &dyn PlatformFs,
    root: &Path,
    options: &PlanOptions,
) -> BTreeMap<OsString, Existing> {
    let mut map = BTreeMap::new();
    if !root.exists() {
        return map;
    }

    let walk = walkdir::WalkDir::new(fs.long_path(root))
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        // Shelv's own trash folder is not part of the backup. Indexing it
        // would make every previously deleted file look extraneous, so the
        // next mirror run would move the trash into the trash, and the run
        // after that would do it again.
        .filter_entry(|entry| entry.file_name() != crate::engine::trash::TRASH_DIR);

    for entry in walk {
        // An unreadable entry on the *destination* side is not a reason to
        // skip a file. Leaving it out of the index means the source copy is
        // planned again, which is wasteful but safe; treating it as
        // extraneous and deleting it would not be.
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(fs.long_path(root)) else {
            continue;
        };
        let metadata = entry.metadata().ok();
        map.insert(
            options.case_sensitivity.fold(relative),
            Existing {
                relative: relative.to_path_buf(),
                bytes: metadata.as_ref().map_or(0, std::fs::Metadata::len),
                modified: metadata.and_then(|m| m.modified().ok()),
            },
        );
    }
    map
}

/// Walks the source, filling the plan and consuming matches from the
/// destination index.
fn walk_source(
    fs: &dyn PlatformFs,
    root: &Path,
    options: &PlanOptions,
    excludes: &GlobSet,
    existing: &mut BTreeMap<OsString, Existing>,
    plan: &mut Plan,
) -> Result<()> {
    let long_root = fs.long_path(root);
    if !long_root.exists() {
        return Err(CoreError::Io(format!(
            "the source folder {} is not there",
            root.display()
        )));
    }

    // `follow_links` stays false whatever the rule says. Following here
    // would let walkdir leave the tree before anything gets a chance to
    // check where it went; links are resolved one at a time below, where
    // the result can be tested for containment.
    let walk = walkdir::WalkDir::new(&long_root)
        .max_depth(MAX_DEPTH)
        .follow_links(false);

    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                // Depth is reported by walkdir as an ordinary error, so the
                // two are told apart by asking it.
                let relative = e
                    .path()
                    .and_then(|p| p.strip_prefix(&long_root).ok())
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let too_deep = e.depth() >= MAX_DEPTH;
                plan.skipped.push(Skipped {
                    relative,
                    reason: if too_deep {
                        SkipReason::TooDeep
                    } else {
                        SkipReason::Unreadable
                    },
                    detail: Some(e.to_string()),
                });
                continue;
            }
        };

        let Ok(relative) = entry.path().strip_prefix(&long_root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }

        if excludes.is_match(relative) {
            plan.skipped.push(Skipped {
                relative: relative.to_path_buf(),
                reason: SkipReason::Excluded,
                detail: None,
            });
            continue;
        }

        // `symlink_metadata` rather than `metadata`: the latter follows the
        // link, and a link is exactly what needs deciding about before
        // anything reads through it.
        let metadata = match std::fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(e) => {
                plan.skipped.push(Skipped {
                    relative: relative.to_path_buf(),
                    reason: SkipReason::Unreadable,
                    detail: Some(e.to_string()),
                });
                continue;
            }
        };

        if metadata.file_type().is_symlink() {
            match resolve_link(entry.path(), &long_root, options.follow_symlinks) {
                Ok(Some(target)) => {
                    // Followed, and it stayed inside. Treat it as whatever
                    // it points at.
                    if target.is_dir() {
                        plan.directories.push(relative.to_path_buf());
                    } else if let Ok(target_meta) = std::fs::metadata(&target) {
                        consider_file(relative, &target_meta, options, existing, plan);
                    }
                }
                Ok(None) => plan.skipped.push(Skipped {
                    relative: relative.to_path_buf(),
                    reason: SkipReason::Symlink,
                    detail: None,
                }),
                Err(reason) => plan.skipped.push(Skipped {
                    relative: relative.to_path_buf(),
                    reason,
                    detail: None,
                }),
            }
            continue;
        }

        if metadata.is_dir() {
            plan.directories.push(relative.to_path_buf());
        } else if metadata.is_file() {
            consider_file(relative, &metadata, options, existing, plan);
        }
        // Anything else — a device node, a socket — is not a file a backup
        // can meaningfully carry, and is passed over without comment.
    }

    Ok(())
}

/// Decides what a link should count as.
///
/// `Ok(None)` means "do not follow, skip it". `Err` means it was refused.
fn resolve_link(
    path: &Path,
    root: &Path,
    follow: bool,
) -> std::result::Result<Option<PathBuf>, SkipReason> {
    if !follow {
        return Ok(None);
    }
    // Canonicalising both ends is the only way to compare them honestly:
    // the root may itself contain links, and a textual prefix test would
    // then reject something that is genuinely inside.
    let Ok(target) = std::fs::canonicalize(path) else {
        return Err(SkipReason::Unreadable);
    };
    let Ok(real_root) = std::fs::canonicalize(root) else {
        return Err(SkipReason::Unreadable);
    };
    if target.starts_with(&real_root) {
        Ok(Some(target))
    } else {
        Err(SkipReason::EscapesSource)
    }
}

/// Adds a file to the plan, or drops it from the deletion candidates when
/// the destination copy is already current.
fn consider_file(
    relative: &Path,
    metadata: &std::fs::Metadata,
    options: &PlanOptions,
    existing: &mut BTreeMap<OsString, Existing>,
    plan: &mut Plan,
) {
    let key = options.case_sensitivity.fold(relative);
    // Removing rather than reading: whatever is left in the map once the
    // source has been walked is what the source no longer has.
    let Some(there) = existing.remove(&key) else {
        push_copy(plan, relative, metadata.len(), CopyReason::Added);
        return;
    };

    if there.bytes != metadata.len() {
        push_copy(plan, relative, metadata.len(), CopyReason::Resized);
        return;
    }

    let newer = match (metadata.modified().ok(), there.modified) {
        (Some(source), Some(destination)) => !within(source, destination, options.mtime_tolerance),
        // A filesystem that will not report a modification time leaves size
        // as the only evidence. Copying again is wasteful; not copying
        // risks a stale backup, so it copies.
        _ => true,
    };

    if newer {
        push_copy(plan, relative, metadata.len(), CopyReason::Modified);
    }
}

fn push_copy(plan: &mut Plan, relative: &Path, bytes: u64, reason: CopyReason) {
    plan.bytes = plan.bytes.saturating_add(bytes);
    plan.copies.push(PlannedCopy {
        relative: relative.to_path_buf(),
        bytes,
        reason,
    });
}

/// Whether two timestamps are the same to within the filesystem's
/// granularity, in either order.
fn within(a: SystemTime, b: SystemTime, tolerance: Duration) -> bool {
    let apart = a
        .duration_since(b)
        .or_else(|_| b.duration_since(a))
        .unwrap_or_default();
    apart <= tolerance
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use std::fs;
    use std::io::Write as _;

    use super::*;
    use crate::platform::host_fs;

    /// Writes a file, creating its parents, with a given mtime offset from
    /// now so the diff can be steered.
    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(contents).unwrap();
    }

    /// Sets a file's modification time, in seconds since the epoch.
    fn set_mtime(path: &Path, secs: u64) {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    fn options() -> PlanOptions {
        PlanOptions {
            layout: Layout::Mirror,
            excludes: Vec::new(),
            follow_symlinks: false,
            case_sensitivity: CaseSensitivity::Sensitive,
            mtime_tolerance: Duration::ZERO,
        }
    }

    fn run(source: &Path, destination: Option<&Path>, options: &PlanOptions) -> Plan {
        plan(host_fs().as_ref(), source, destination, options).unwrap()
    }

    fn copied(plan: &Plan) -> Vec<(String, CopyReason)> {
        plan.copies
            .iter()
            .map(|c| (c.relative.display().to_string(), c.reason))
            .collect()
    }

    #[test]
    fn everything_is_an_addition_when_the_destination_is_empty() {
        let source = tempfile::tempdir().unwrap();
        write(source.path(), "notes.txt", b"hello");
        write(source.path(), "photos/one.raw", b"12345");

        let plan = run(source.path(), None, &options());

        assert_eq!(plan.copies.len(), 2);
        assert!(plan.copies.iter().all(|c| c.reason == CopyReason::Added));
        assert_eq!(plan.bytes, 10);
        assert!(plan.deletions.is_empty());
    }

    #[test]
    fn an_identical_file_is_not_copied_again() {
        // The whole point of diffing. Re-copying an unchanged tree on every
        // run would make a scheduled backup useless on a large source.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "notes.txt", b"hello");
        write(destination.path(), "notes.txt", b"hello");
        set_mtime(&source.path().join("notes.txt"), 1_700_000_000);
        set_mtime(&destination.path().join("notes.txt"), 1_700_000_000);

        let plan = run(source.path(), Some(destination.path()), &options());

        assert!(plan.copies.is_empty(), "{:?}", copied(&plan));
        assert!(plan.deletions.is_empty());
    }

    #[test]
    fn a_file_that_changed_size_is_copied() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "notes.txt", b"hello there");
        write(destination.path(), "notes.txt", b"hello");
        // Same mtime, so only the size can be what gives it away.
        set_mtime(&source.path().join("notes.txt"), 1_700_000_000);
        set_mtime(&destination.path().join("notes.txt"), 1_700_000_000);

        let plan = run(source.path(), Some(destination.path()), &options());

        assert_eq!(
            copied(&plan),
            vec![("notes.txt".to_owned(), CopyReason::Resized)]
        );
    }

    #[test]
    fn a_file_that_changed_only_its_timestamp_is_copied() {
        // Same length, different contents — the common case for an edited
        // document, and invisible to a size comparison alone.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "notes.txt", b"hello");
        write(destination.path(), "notes.txt", b"world");
        set_mtime(&source.path().join("notes.txt"), 1_700_000_500);
        set_mtime(&destination.path().join("notes.txt"), 1_700_000_000);

        let plan = run(source.path(), Some(destination.path()), &options());

        assert_eq!(
            copied(&plan),
            vec![("notes.txt".to_owned(), CopyReason::Modified)]
        );
    }

    #[test]
    fn a_coarse_destination_clock_does_not_make_every_file_look_changed() {
        // FAT32 keeps mtimes to two seconds. Without the tolerance an NTFS
        // source one second newer looks modified, so the entire tree is
        // re-copied on every single run, forever (`docs/PLAN.md` §1.1f).
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "notes.txt", b"hello");
        write(destination.path(), "notes.txt", b"hello");
        set_mtime(&source.path().join("notes.txt"), 1_700_000_001);
        set_mtime(&destination.path().join("notes.txt"), 1_700_000_000);

        let mut coarse = options();
        coarse.mtime_tolerance = Duration::from_secs(2);
        assert!(run(source.path(), Some(destination.path()), &coarse)
            .copies
            .is_empty());

        // And the tolerance is a tolerance, not a blindfold: three seconds
        // apart is still a change.
        set_mtime(&source.path().join("notes.txt"), 1_700_000_003);
        assert_eq!(
            run(source.path(), Some(destination.path()), &coarse)
                .copies
                .len(),
            1
        );
    }

    #[test]
    fn a_destination_file_the_source_no_longer_has_is_a_deletion() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "kept.txt", b"a");
        write(destination.path(), "kept.txt", b"a");
        write(destination.path(), "gone.txt", b"bb");
        set_mtime(&source.path().join("kept.txt"), 1_700_000_000);
        set_mtime(&destination.path().join("kept.txt"), 1_700_000_000);

        let plan = run(source.path(), Some(destination.path()), &options());

        assert_eq!(plan.deletions.len(), 1);
        assert_eq!(plan.deletions[0].relative, PathBuf::from("gone.txt"));
        assert_eq!(plan.deletions[0].bytes, 2);
    }

    #[test]
    fn a_snapshot_never_deletes_anything() {
        // A past snapshot is a record of a moment. Editing one is not a
        // backup, so the destination is not even looked at.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "kept.txt", b"a");
        write(destination.path(), "gone.txt", b"bb");

        let mut snapshot = options();
        snapshot.layout = Layout::Snapshot;
        let plan = run(source.path(), Some(destination.path()), &snapshot);

        assert!(plan.deletions.is_empty());
        // And it copies everything, because a snapshot is always a fresh
        // tree rather than a difference against an old one.
        assert_eq!(
            copied(&plan),
            vec![("kept.txt".to_owned(), CopyReason::Added)]
        );
    }

    #[test]
    fn excluded_files_are_left_out_and_named() {
        let source = tempfile::tempdir().unwrap();
        write(source.path(), "notes.txt", b"a");
        write(source.path(), "scratch.tmp", b"b");

        let mut opts = options();
        opts.excludes = vec!["*.tmp".to_owned()];
        let plan = run(source.path(), None, &opts);

        assert_eq!(
            copied(&plan),
            vec![("notes.txt".to_owned(), CopyReason::Added)]
        );
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::Excluded);
        assert_eq!(plan.skipped[0].relative, PathBuf::from("scratch.tmp"));
    }

    #[test]
    fn an_unparseable_ignore_pattern_is_refused_rather_than_dropped() {
        // Silently ignoring it would back up something the user asked to
        // leave out, and they would have no way to notice.
        let source = tempfile::tempdir().unwrap();
        let mut opts = options();
        opts.excludes = vec!["[".to_owned()];

        let error = plan(host_fs().as_ref(), source.path(), None, &opts).unwrap_err();
        assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    }

    #[test]
    fn an_empty_source_folder_still_appears_in_the_backup() {
        let source = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("empty")).unwrap();

        let plan = run(source.path(), None, &options());

        assert_eq!(plan.directories, vec![PathBuf::from("empty")]);
    }

    #[test]
    fn a_missing_source_is_an_error_rather_than_an_empty_plan() {
        // An empty plan would read as "nothing to do" and the run would be
        // recorded as a success, which is the worst possible answer when
        // the folder has been moved or deleted.
        let source = tempfile::tempdir().unwrap();
        let missing = source.path().join("not-there");

        let error = plan(host_fs().as_ref(), &missing, None, &options()).unwrap_err();
        assert!(matches!(error, CoreError::Io(_)), "{error:?}");
    }

    // Unix only, because the situation cannot be built on Windows: NTFS
    // will not hold `Photo.RAW` and `photo.raw` in one directory, so the
    // second write replaces the first and the test would be asserting
    // against a tree it failed to create. What is under test is the
    // `CaseSensitivity::Sensitive` branch, which is ordinary Rust and is
    // exercised here on a filesystem that can actually represent it. Its
    // companion below — the insensitive branch, which is the one Windows
    // actually takes — puts the two names in *different* directories and
    // runs everywhere.
    #[cfg(unix)]
    #[test]
    fn two_names_differing_only_in_case_do_not_collide_on_a_sensitive_volume() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "Photo.RAW", b"a");
        write(source.path(), "photo.raw", b"b");
        write(destination.path(), "Photo.RAW", b"a");
        set_mtime(&source.path().join("Photo.RAW"), 1_700_000_000);
        set_mtime(&destination.path().join("Photo.RAW"), 1_700_000_000);

        let plan = run(source.path(), Some(destination.path()), &options());

        // Only the lower-case one is new, and nothing is extraneous.
        assert_eq!(
            copied(&plan),
            vec![("photo.raw".to_owned(), CopyReason::Added)]
        );
        assert!(plan.deletions.is_empty());
    }

    #[test]
    fn a_case_insensitive_destination_does_not_delete_the_file_it_just_matched() {
        // The failure this guards against: the source has `Photo.RAW`, the
        // destination has `photo.raw`, and folding them differently leaves
        // mirror copying one and then deleting the other as extraneous —
        // losing the file it had just written.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        write(source.path(), "Photo.RAW", b"a");
        write(destination.path(), "photo.raw", b"a");
        set_mtime(&source.path().join("Photo.RAW"), 1_700_000_000);
        set_mtime(&destination.path().join("photo.raw"), 1_700_000_000);

        let mut insensitive = options();
        insensitive.case_sensitivity = CaseSensitivity::Insensitive;
        let plan = run(source.path(), Some(destination.path()), &insensitive);

        assert!(plan.copies.is_empty(), "{:?}", copied(&plan));
        assert!(
            plan.deletions.is_empty(),
            "the matched file must not also be a deletion candidate"
        );
    }

    #[test]
    fn unreadable_entries_make_the_run_partial_rather_than_ok() {
        let mut plan = Plan::default();
        assert!(!plan.has_unreadable());
        plan.skipped.push(Skipped {
            relative: PathBuf::from("locked.db"),
            reason: SkipReason::Unreadable,
            detail: None,
        });
        assert!(plan.has_unreadable());

        // An exclusion is a choice, not a failure, and must not downgrade a
        // run that otherwise copied everything.
        let mut excluded = Plan::default();
        excluded.skipped.push(Skipped {
            relative: PathBuf::from("scratch.tmp"),
            reason: SkipReason::Excluded,
            detail: None,
        });
        assert!(!excluded.has_unreadable());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_tree_is_refused_even_when_links_are_followed() {
        // The escape this exists to stop: a link dropped inside the source
        // is otherwise a way to make Shelv copy anything on the machine
        // (`docs/PLAN.md` §4, T2).
        let source = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", b"private");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            source.path().join("link"),
        )
        .unwrap();

        let mut following = options();
        following.follow_symlinks = true;
        let plan = run(source.path(), None, &following);

        assert!(plan.copies.is_empty(), "{:?}", copied(&plan));
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::EscapesSource);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_tree_is_copied_when_links_are_followed() {
        let source = tempfile::tempdir().unwrap();
        write(source.path(), "real.txt", b"hello");
        std::os::unix::fs::symlink(source.path().join("real.txt"), source.path().join("link"))
            .unwrap();

        let mut following = options();
        following.follow_symlinks = true;
        let plan = run(source.path(), None, &following);

        let mut names: Vec<_> = plan.copies.iter().map(|c| c.relative.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![PathBuf::from("link"), PathBuf::from("real.txt")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_are_passed_over_by_default() {
        // Not following is the default because reading through a link is
        // how a copy leaves its tree, and `fs::read` follows one whether or
        // not the walk did.
        let source = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", b"private");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            source.path().join("link"),
        )
        .unwrap();

        let plan = run(source.path(), None, &options());

        assert!(plan.copies.is_empty());
        assert_eq!(plan.skipped[0].reason, SkipReason::Symlink);
    }
}

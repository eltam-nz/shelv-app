//! Writing a plan to the destination.
//!
//! Everything here creates; nothing here removes. Mirror's deletions are a
//! separate step (`docs/M1.md`, M1.3), kept apart so the code that can
//! destroy a file is reviewed on its own rather than buried among the code
//! that cannot.
//!
//! The whole design turns on one requirement: **pulling the drive mid-copy
//! must never leave a truncated file where a good one was**
//! (`docs/PLAN.md` §5.1). So every file is written to a temporary name on
//! the destination volume and renamed into place only once its bytes are on
//! disk. A rename within a volume is atomic, so at no instant does the
//! destination hold a half-written file under its real name: it holds either
//! the previous complete copy or the new complete one.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::planner::Plan;
use crate::platform::PlatformFs;
use crate::{CoreError, Result};

/// How many files are copied at once.
///
/// Deliberately low. Backup destinations are overwhelmingly external disks,
/// and oversubscribing a mechanical one makes it slower rather than faster:
/// the heads spend their time seeking between interleaved files instead of
/// reading. Four is enough to keep a queue busy through per-file overhead
/// without turning a sequential write into a random one.
pub const DEFAULT_WORKERS: usize = 4;

/// Read and write size.
///
/// Large enough that syscall overhead disappears against the transfer, small
/// enough that cancellation is felt within a few milliseconds even mid-file.
const CHUNK: usize = 1024 * 1024;

/// The prefix every temporary file carries.
///
/// Named so that a temp file left behind by a process that was killed
/// outright is recognisable as Shelv's, and so the planner's own walk of a
/// destination could exclude them.
const TEMP_PREFIX: &str = ".shelv-tmp-";

/// One file that could not be copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct CopyFailure {
    /// Path relative to the source root.
    pub relative: PathBuf,
    /// What went wrong, in the words the OS used.
    pub message: String,
}

/// What a copy actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct CopyReport {
    /// Files written and renamed into place.
    #[ts(type = "number")]
    pub files_copied: u64,
    /// Bytes written.
    #[ts(type = "number")]
    pub bytes_copied: u64,
    /// Directories created.
    #[ts(type = "number")]
    pub directories_created: u64,
    /// Files that could not be read or written. Each one means the run is
    /// `Partial`, never `Ok`.
    pub failures: Vec<CopyFailure>,
    /// Whether the run stopped because it was asked to.
    pub cancelled: bool,
}

impl CopyReport {
    /// Whether every file in the plan was written.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && !self.cancelled
    }
}

/// What the copier reports as it goes, and how it is stopped.
///
/// A trait rather than a channel because `shelv-core` owns no runtime: the
/// Tauri shell implements this and turns the calls into throttled events,
/// exactly as `watcher.rs` already does for volume changes.
///
/// Every method is called from worker threads, so implementations must be
/// prepared for concurrent calls and must not block for long — a slow
/// observer slows the backup.
pub trait CopyObserver: Sync {
    /// The plan for one destination is known: this many files, this many
    /// bytes.
    ///
    /// Called once per destination rather than once per run, because the
    /// plan is made per destination — a rule aimed at two drives has two of
    /// them, and the totals a progress bar shows grow as the run moves from
    /// one to the next.
    fn planned(&self, _files: u64, _bytes: u64) {}

    /// A file is about to be written.
    fn file_started(&self, _relative: &Path, _bytes: u64) {}

    /// Another `delta` bytes have been written.
    fn bytes_written(&self, _delta: u64) {}

    /// A file has been renamed into place.
    fn file_finished(&self, _relative: &Path) {}

    /// Whether the run should stop.
    ///
    /// Checked between chunks as well as between files, so cancelling during
    /// a large file takes effect without waiting for it to finish. Whatever
    /// was being written is a temp file, so abandoning it leaves the
    /// destination exactly as it was.
    fn cancelled(&self) -> bool {
        false
    }
}

/// An observer that reports nothing and never cancels, for tests and for
/// callers that only want the report at the end.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl CopyObserver for Silent {}

/// Writes a plan's copies and directories beneath `destination`.
///
/// `source` and `destination` are the two roots; every path in the plan is
/// relative to one of them, so nothing here trusts a path it was handed
/// whole.
///
/// Returns `Err` only when the copy could not be attempted at all — an
/// unwritable destination root. A file that fails individually is recorded
/// in [`CopyReport::failures`] and the rest of the run continues, because a
/// backup that stops at the first locked file backs up almost nothing.
pub fn copy_plan(
    fs: &dyn PlatformFs,
    plan: &Plan,
    source: &Path,
    destination: &Path,
    observer: &dyn CopyObserver,
) -> Result<CopyReport> {
    copy_plan_with_workers(fs, plan, source, destination, observer, DEFAULT_WORKERS)
}

/// [`copy_plan`], with the worker count chosen by the caller. Tests use one
/// worker so that ordering is deterministic.
pub fn copy_plan_with_workers(
    fs: &dyn PlatformFs,
    plan: &Plan,
    source: &Path,
    destination: &Path,
    observer: &dyn CopyObserver,
    workers: usize,
) -> Result<CopyReport> {
    let state = State::default();

    std::fs::create_dir_all(fs.long_path(destination)).map_err(|e| {
        CoreError::Io(format!(
            "could not create the destination folder {}: {e}",
            destination.display()
        ))
    })?;

    // Directories first, and on this thread: they are cheap, they are
    // ordered parents-before-children by the walk, and creating them up
    // front means a worker never races another to create the same parent.
    for relative in &plan.directories {
        if observer.cancelled() {
            state.cancelled.store(true, Ordering::Relaxed);
            break;
        }
        match std::fs::create_dir_all(fs.long_path(&destination.join(relative))) {
            Ok(()) => {
                state.directories.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => state.fail(relative, &e.to_string()),
        }
    }

    let next = AtomicUsize::new(0);
    let worker = || {
        while let Some(copy) = plan.copies.get(next.fetch_add(1, Ordering::Relaxed)) {
            if observer.cancelled() {
                state.cancelled.store(true, Ordering::Relaxed);
                return;
            }
            observer.file_started(&copy.relative, copy.bytes);
            match copy_one(fs, source, destination, &copy.relative, observer) {
                Ok(Written::Complete(bytes)) => {
                    state.files.fetch_add(1, Ordering::Relaxed);
                    state.bytes.fetch_add(bytes, Ordering::Relaxed);
                    observer.file_finished(&copy.relative);
                }
                Ok(Written::Abandoned) => {
                    state.cancelled.store(true, Ordering::Relaxed);
                    return;
                }
                Err(e) => state.fail(&copy.relative, &e.to_string()),
            }
        }
    };

    // One worker runs inline rather than spawning a thread to sit idle,
    // which also keeps the single-threaded path — the one tests use — free
    // of any scheduling at all.
    if workers <= 1 {
        worker();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(worker);
            }
        });
    }

    Ok(state.into_report())
}

/// Whether a file made it into place.
enum Written {
    /// Renamed into place, with its byte count.
    Complete(u64),
    /// Cancelled part-way. The temp file has been removed and the
    /// destination is untouched.
    Abandoned,
}

/// Copies one file: temp file, bytes, timestamp, rename.
fn copy_one(
    fs: &dyn PlatformFs,
    source: &Path,
    destination: &Path,
    relative: &Path,
    observer: &dyn CopyObserver,
) -> std::io::Result<Written> {
    let from = fs.long_path(&source.join(relative));
    let to = destination.join(relative);
    let long_to = fs.long_path(&to);

    if let Some(parent) = long_to.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut reader = File::open(&from)?;
    // Read before writing, so a file whose timestamp cannot be read fails
    // without having touched the destination.
    let modified = reader.metadata()?.modified()?;

    let temp = temp_path(fs, &to);
    let outcome = write_temp(&mut reader, &temp, modified, observer);

    match outcome {
        Ok(Some(bytes)) => {
            // The rename is the moment the new file becomes visible, and it
            // is atomic within a volume. Everything before it was a file
            // nothing refers to.
            std::fs::rename(&temp, &long_to).inspect_err(|_| {
                let _ = std::fs::remove_file(&temp);
            })?;
            Ok(Written::Complete(bytes))
        }
        Ok(None) => {
            let _ = std::fs::remove_file(&temp);
            Ok(Written::Abandoned)
        }
        Err(e) => {
            // A failed write must not leave its temp file behind, or a
            // destination accumulates debris from every interrupted run.
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Streams `reader` into `temp`, returning the byte count, or `None` if the
/// copy was cancelled part-way.
fn write_temp(
    reader: &mut File,
    temp: &Path,
    modified: std::time::SystemTime,
    observer: &dyn CopyObserver,
) -> std::io::Result<Option<u64>> {
    let mut writer = File::create(temp)?;
    let mut buffer = vec![0_u8; CHUNK];
    let mut total = 0_u64;

    loop {
        if observer.cancelled() {
            return Ok(None);
        }
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).unwrap_or_default();
        writer.write_all(chunk)?;
        total += read as u64;
        observer.bytes_written(read as u64);
    }

    // The timestamp is not decoration: the planner diffs on size and mtime,
    // so a copy that lands with the time of the copy rather than the time of
    // the original looks newer than its source forever and is re-copied on
    // every single run.
    writer.set_modified(modified)?;

    // Bytes on the platter before the rename, or a power cut between the two
    // leaves a file that exists under its real name with nothing in it —
    // precisely what temp-and-rename is here to prevent.
    writer.sync_all()?;
    Ok(Some(total))
}

/// A temporary name beside the destination file.
///
/// Beside it, not in a temp directory, for two reasons: the rename must stay
/// within one volume to be atomic, and a temp file on the system drive would
/// mean the destination drive never sees the bytes until a slow cross-device
/// copy at the end.
///
/// The process id and a counter keep two workers, or two Shelv instances,
/// off each other's names.
fn temp_path(fs: &dyn PlatformFs, destination: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .map_or_else(|| "file".into(), std::ffi::OsStr::to_os_string);

    let mut temp = std::ffi::OsString::from(TEMP_PREFIX);
    temp.push(std::process::id().to_string());
    temp.push("-");
    temp.push(n.to_string());
    temp.push("-");
    temp.push(name);

    fs.long_path(&destination.with_file_name(temp))
}

/// Counters shared across workers.
#[derive(Default)]
struct State {
    files: AtomicU64,
    bytes: AtomicU64,
    directories: AtomicU64,
    cancelled: std::sync::atomic::AtomicBool,
    failures: Mutex<Vec<CopyFailure>>,
}

impl State {
    fn fail(&self, relative: &Path, message: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.push(CopyFailure {
                relative: relative.to_path_buf(),
                message: message.to_owned(),
            });
        }
    }

    fn into_report(self) -> CopyReport {
        CopyReport {
            files_copied: self.files.into_inner(),
            bytes_copied: self.bytes.into_inner(),
            directories_created: self.directories.into_inner(),
            failures: self.failures.into_inner().unwrap_or_default(),
            cancelled: self.cancelled.into_inner(),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::engine::planner::{plan, PlanOptions};
    use crate::model::Layout;
    use crate::platform::{CaseSensitivity, SpaceInfo, VolumeInfo};

    /// A platform that knows nothing about volumes, since the copier only
    /// ever asks it to rewrite paths.
    struct FakeFs;

    impl PlatformFs for FakeFs {
        fn volumes(&self) -> Result<Vec<VolumeInfo>> {
            Ok(Vec::new())
        }

        fn volume_of(&self, _path: &Path) -> Result<VolumeInfo> {
            Err(CoreError::VolumeUnavailable("no volumes".into()))
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

    fn options() -> PlanOptions {
        PlanOptions {
            layout: Layout::Mirror,
            excludes: Vec::new(),
            follow_symlinks: false,
            case_sensitivity: CaseSensitivity::Sensitive,
            mtime_tolerance: std::time::Duration::ZERO,
        }
    }

    /// Plans and then copies, which is how a real run works: the copier is
    /// never handed a plan from anywhere else.
    fn run(src: &Path, dst: &Path) -> CopyReport {
        run_with(src, dst, &Silent)
    }

    fn run_with(src: &Path, dst: &Path, observer: &dyn CopyObserver) -> CopyReport {
        let fs = FakeFs;
        let plan = plan(&fs, src, Some(dst), &options()).unwrap();
        copy_plan_with_workers(&fs, &plan, src, dst, observer, 1).unwrap()
    }

    /// Cancels once `after` bytes have gone by, so a copy can be stopped in
    /// the middle of a file rather than between two of them.
    struct CancelAfter {
        after: u64,
        seen: AtomicU64,
        flag: AtomicBool,
    }

    impl CancelAfter {
        fn new(after: u64) -> Self {
            Self {
                after,
                seen: AtomicU64::new(0),
                flag: AtomicBool::new(false),
            }
        }
    }

    impl CopyObserver for CancelAfter {
        fn bytes_written(&self, delta: u64) {
            if self.seen.fetch_add(delta, Ordering::Relaxed) + delta >= self.after {
                self.flag.store(true, Ordering::Relaxed);
            }
        }

        fn cancelled(&self) -> bool {
            self.flag.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn a_file_arrives_with_its_contents_and_its_timestamp() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"hello").unwrap();

        let report = run(src.path(), dst.path());

        assert_eq!(report.files_copied, 1);
        assert_eq!(report.bytes_copied, 5);
        assert!(report.is_complete());
        assert_eq!(
            std::fs::read(dst.path().join("one.raw")).unwrap(),
            b"hello".to_vec()
        );

        // The mtime has to survive the copy, or the planner sees a
        // destination newer than its source and re-copies the file on every
        // run for the rest of the rule's life.
        let source_time = std::fs::metadata(src.path().join("one.raw"))
            .unwrap()
            .modified()
            .unwrap();
        let copied_time = std::fs::metadata(dst.path().join("one.raw"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(source_time, copied_time);
    }

    #[test]
    fn a_copied_tree_plans_as_nothing_to_do_on_the_second_run() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("raw/2026")).unwrap();
        std::fs::write(src.path().join("raw/2026/a.dng"), b"aaaa").unwrap();
        std::fs::write(src.path().join("b.txt"), b"bb").unwrap();

        assert_eq!(run(src.path(), dst.path()).files_copied, 2);

        // The real proof that the copy was faithful: the planner, run again,
        // finds nothing to do. Size and mtime both have to match for that.
        let second = plan(&FakeFs, src.path(), Some(dst.path()), &options()).unwrap();
        assert!(second.copies.is_empty(), "{:?}", second.copies);
        assert!(second.deletions.is_empty(), "{:?}", second.deletions);
    }

    #[test]
    fn an_empty_source_folder_is_created_at_the_destination() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("empty")).unwrap();

        let report = run(src.path(), dst.path());

        assert!(dst.path().join("empty").is_dir());
        assert_eq!(report.directories_created, 1);
    }

    #[test]
    fn cancelling_mid_file_leaves_the_previous_copy_intact() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // A file big enough to span several chunks, so the cancellation
        // lands in the middle of writing it rather than between files.
        let big = vec![b'n'; CHUNK * 3];
        std::fs::write(src.path().join("big.raw"), &big).unwrap();
        // The good copy that must survive being interrupted.
        std::fs::write(dst.path().join("big.raw"), b"the previous backup").unwrap();

        let observer = CancelAfter::new(CHUNK as u64);
        let report = run_with(src.path(), dst.path(), &observer);

        assert!(report.cancelled);
        assert_eq!(report.files_copied, 0);
        assert!(!report.is_complete());

        // This is the acceptance criterion: what is under the real name is
        // still the complete previous file, not a truncated new one.
        assert_eq!(
            std::fs::read(dst.path().join("big.raw")).unwrap(),
            b"the previous backup".to_vec()
        );
    }

    #[test]
    fn a_cancelled_copy_leaves_no_temporary_file_behind() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("big.raw"), vec![b'n'; CHUNK * 3]).unwrap();

        let observer = CancelAfter::new(CHUNK as u64);
        run_with(src.path(), dst.path(), &observer);

        let leftovers: Vec<_> = std::fs::read_dir(dst.path())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|name| name.to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_recorded_and_the_rest_of_the_run_continues() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), b"first").unwrap();
        std::fs::write(src.path().join("vanishes.txt"), b"second").unwrap();
        std::fs::write(src.path().join("z.txt"), b"third").unwrap();

        let fs = FakeFs;
        let planned = plan(&fs, src.path(), Some(dst.path()), &options()).unwrap();

        // A file that is gone by the time the copier reaches it. Planning
        // and copying cannot be one atomic act against a live filesystem, so
        // this is the ordinary case, not a contrived one — a temp file the
        // source application cleaned up, or a download that moved.
        std::fs::remove_file(src.path().join("vanishes.txt")).unwrap();

        let report =
            copy_plan_with_workers(&fs, &planned, src.path(), dst.path(), &Silent, 1).unwrap();

        // A backup that stops at the first unreadable file backs up almost
        // nothing, so the other two must still be there.
        assert!(dst.path().join("a.txt").exists());
        assert!(dst.path().join("z.txt").exists());
        assert_eq!(report.files_copied, 2);

        // And the failure is named rather than swallowed: this is what makes
        // the run Partial instead of Ok.
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].relative, PathBuf::from("vanishes.txt"));
        assert!(!report.is_complete());
    }

    #[test]
    fn an_existing_destination_file_is_replaced_rather_than_appended_to() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("one.raw"), b"new").unwrap();
        std::fs::write(dst.path().join("one.raw"), b"much older and longer").unwrap();

        run(src.path(), dst.path());

        assert_eq!(
            std::fs::read(dst.path().join("one.raw")).unwrap(),
            b"new".to_vec()
        );
    }

    #[test]
    fn several_workers_copy_the_same_tree_as_one_does() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        for i in 0..40 {
            std::fs::create_dir_all(src.path().join(format!("dir{}", i % 4))).unwrap();
            std::fs::write(
                src.path().join(format!("dir{}/file{i}.bin", i % 4)),
                format!("contents of {i}"),
            )
            .unwrap();
        }

        let fs = FakeFs;
        let planned = plan(&fs, src.path(), Some(dst.path()), &options()).unwrap();
        let report =
            copy_plan_with_workers(&fs, &planned, src.path(), dst.path(), &Silent, 4).unwrap();

        assert_eq!(report.files_copied, 40);
        assert!(report.is_complete(), "{:?}", report.failures);

        // No two workers may have collided over a temp name or a parent
        // directory, which shows up as the second run finding work to do.
        let second = plan(&fs, src.path(), Some(dst.path()), &options()).unwrap();
        assert!(second.copies.is_empty(), "{:?}", second.copies);
    }

    #[test]
    fn nothing_is_removed_from_the_destination_even_when_the_plan_says_to() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("kept.txt"), b"kept").unwrap();
        std::fs::write(dst.path().join("extraneous.txt"), b"still here").unwrap();

        let fs = FakeFs;
        let planned = plan(&fs, src.path(), Some(dst.path()), &options()).unwrap();
        assert_eq!(planned.deletions.len(), 1, "the plan does call for it");

        copy_plan_with_workers(&fs, &planned, src.path(), dst.path(), &Silent, 1).unwrap();

        // The copier creates and nothing else. Deletion is M1.3, and keeping
        // it out of here is what lets that step be reviewed on its own.
        assert!(dst.path().join("extraneous.txt").exists());
    }
}

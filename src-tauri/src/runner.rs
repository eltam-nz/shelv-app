//! Running a backup on a thread, and reporting it to the window.
//!
//! The engine in `shelv-core` is synchronous and knows nothing of Tauri.
//! This is the piece that gives it a thread to run on, turns its callbacks
//! into events, and lets the window stop it.
//!
//! One run at a time, deliberately. Two runs could otherwise write to one
//! drive at once, and a second Backup Now on a rule already running would
//! copy the same tree twice. The scheduler in M3 will queue runs per volume;
//! until then, refusing the second is the honest behaviour.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use shelv_core::engine::{CopyObserver, RunFinished, RunProgress};
use shelv_core::model::{RuleId, RunTrigger};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::commands::AppState;

/// Progress, several times a second while a run is going.
pub const RUN_PROGRESS: &str = "shelv://run-progress";

/// Emitted once, whatever the outcome — including a failure, so the window
/// never waits forever for a run that has already stopped.
pub const RUN_FINISHED: &str = "shelv://run-finished";

/// How often progress reaches the window.
///
/// A million-file run would otherwise emit a million events and spend its
/// time serialising them rather than copying. Ten a second is faster than
/// anyone can read and slow enough to cost nothing.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The run currently going, if any.
#[derive(Debug, Default)]
pub struct Runner {
    current: Mutex<Option<Current>>,
}

/// A run in progress.
#[derive(Debug, Clone)]
struct Current {
    rule: RuleId,
    cancel: Arc<AtomicBool>,
}

impl Runner {
    /// The rule currently running, if any.
    ///
    /// The window asks on mount, so that reloading it mid-run shows the run
    /// rather than an idle table.
    pub fn running(&self) -> Option<RuleId> {
        self.current
            .lock()
            .ok()
            .and_then(|current| current.as_ref().map(|c| c.rule))
    }

    /// Asks the running backup to stop. Does nothing if none is going.
    pub fn cancel(&self) {
        if let Ok(current) = self.current.lock() {
            if let Some(current) = current.as_ref() {
                current.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Claims the runner for `rule`, or reports what is already running.
    fn claim(&self, rule: RuleId) -> Result<Arc<AtomicBool>, RuleId> {
        let mut current = self.locked();
        if let Some(existing) = current.as_ref() {
            return Err(existing.rule);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *current = Some(Current {
            rule,
            cancel: Arc::clone(&cancel),
        });
        Ok(cancel)
    }

    /// Releases the runner, whatever happened to the run.
    fn release(&self) {
        *self.locked() = None;
    }

    /// The slot, recovering from a poisoned lock rather than propagating it.
    ///
    /// A panic in a run thread must not leave Backup Now permanently
    /// refusing every rule for the rest of the session.
    fn locked(&self) -> std::sync::MutexGuard<'_, Option<Current>> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Starts a backup on a background thread.
///
/// Returns as soon as the thread is running, so the window never blocks on a
/// copy. Everything after that reaches the frontend as an event.
///
/// Refuses while another run is going: two runs could otherwise write to one
/// drive at once (`docs/ARCHITECTURE.md`, Concurrency).
pub fn start<R: Runtime>(app: &AppHandle<R>, rule: RuleId) -> shelv_core::Result<()> {
    let state = app.state::<AppState>();
    let cancel = state.runner.claim(rule).map_err(|busy| {
        shelv_core::CoreError::Refused(if busy == rule {
            "that backup is already running".to_owned()
        } else {
            "another backup is running. Shelv runs one at a time so two rules \
             cannot write to one drive at once."
                .to_owned()
        })
    })?;

    let handle = app.clone();
    thread::Builder::new()
        .name("shelv-backup".to_owned())
        .spawn(move || {
            let observer = Reporter::new(&handle, rule, Arc::clone(&cancel));
            let state = handle.state::<AppState>();

            let outcome = state.run(rule, RunTrigger::Manual, &observer);

            // Released before the event, so a frontend that starts another
            // run the moment it hears "finished" is not refused for a run
            // that has already ended.
            state.runner.release();

            let finished = match outcome {
                Ok(summaries) => RunFinished {
                    rule,
                    summaries,
                    error: None,
                },
                Err(e) => RunFinished {
                    rule,
                    summaries: Vec::new(),
                    error: Some(e.to_string()),
                },
            };
            if let Err(e) = handle.emit(RUN_FINISHED, &finished) {
                tracing::warn!(error = %e, "could not tell the window the run finished");
            }
        })
        .map_err(|e| {
            // The claim has to be given back, or a failure to spawn leaves
            // Backup Now refusing every rule until the app restarts.
            app_state_release(app);
            shelv_core::CoreError::Io(format!("could not start the backup thread: {e}"))
        })?;

    Ok(())
}

/// Releases the runner on an app handle. Split out so the spawn failure path
/// stays readable.
fn app_state_release<R: Runtime>(app: &AppHandle<R>) {
    app.state::<AppState>().runner.release();
}

/// Turns the engine's callbacks into events, at a rate the window can take.
struct Reporter<R: Runtime> {
    app: AppHandle<R>,
    rule: RuleId,
    cancel: Arc<AtomicBool>,
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    files_total: AtomicU64,
    bytes_total: AtomicU64,
    file: Mutex<String>,
    last_emit: Mutex<Instant>,
}

impl<R: Runtime> Reporter<R> {
    fn new(app: &AppHandle<R>, rule: RuleId, cancel: Arc<AtomicBool>) -> Self {
        Self {
            app: app.clone(),
            rule,
            cancel,
            files_done: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            file: Mutex::new(String::new()),
            // Far enough in the past that the first call emits at once,
            // and checked rather than subtracted: `Instant` has no defined
            // zero, so on a machine up for less than a tenth of a second
            // the subtraction would be the one that panics.
            last_emit: Mutex::new(
                Instant::now()
                    .checked_sub(PROGRESS_INTERVAL)
                    .unwrap_or_else(Instant::now),
            ),
        }
    }

    /// Emits progress if enough time has passed since the last one.
    fn emit(&self, force: bool) {
        {
            let mut last = self
                .last_emit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !force && last.elapsed() < PROGRESS_INTERVAL {
                return;
            }
            *last = Instant::now();
        }

        let file = self
            .file
            .lock()
            .map_or_else(|e| e.into_inner().clone(), |f| f.clone());

        let progress = RunProgress {
            rule: self.rule,
            file,
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
        };
        if let Err(e) = self.app.emit(RUN_PROGRESS, &progress) {
            tracing::warn!(error = %e, "could not report progress to the window");
        }
    }
}

impl<R: Runtime> CopyObserver for Reporter<R> {
    fn planned(&self, files: u64, bytes: u64) {
        // Added to rather than set: a rule with two destinations plans each
        // one as it reaches it, so the totals grow as the run moves between
        // drives. A bar that reset at each drive would look like it had gone
        // backwards.
        self.files_total.fetch_add(files, Ordering::Relaxed);
        self.bytes_total.fetch_add(bytes, Ordering::Relaxed);
        self.emit(true);
    }

    fn file_started(&self, relative: &Path, bytes: u64) {
        // Lossy on purpose: this is a label on a progress line, never a path
        // anything reopens. A filename with an unpaired surrogate should
        // show as best it can rather than stop the display.
        #[allow(clippy::disallowed_methods)]
        let name = relative.to_string_lossy().into_owned();
        if let Ok(mut current) = self.file.lock() {
            *current = name;
        }
        let _ = bytes;
        self.emit(false);
    }

    fn bytes_written(&self, delta: u64) {
        self.bytes_done.fetch_add(delta, Ordering::Relaxed);
        self.emit(false);
    }

    fn file_finished(&self, _relative: &Path) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.emit(false);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

//! Running a backup on a thread, and reporting it to the window.
//!
//! The engine in `shelv-core` is synchronous and knows nothing of Tauri.
//! This is the piece that gives it a thread to run on, turns its callbacks
//! into events, and lets the window stop it.
//!
//! One run at a time, deliberately: two runs could otherwise write to one
//! drive at once, and a second Backup Now on a rule already running would
//! copy the same tree twice.
//!
//! **Scheduled runs queue; manual ones do not.** The scheduler has nobody
//! to tell, so a rule it wants started waits its turn. Backup Now has
//! somebody watching, and a place in a line whose length they cannot see is
//! worse than an answer: it is refused while another run is going, and says
//! so.

use std::collections::VecDeque;
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

/// The run currently going, and the ones waiting.
#[derive(Debug, Default)]
pub struct Runner {
    current: Mutex<Option<Current>>,
    /// Rules the scheduler wants started, oldest first.
    pending: Mutex<VecDeque<(RuleId, RunTrigger)>>,
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

    /// Adds a rule the scheduler wants run, if it is not already queued or
    /// running.
    ///
    /// Deduplicated by rule: a rule that is due *and* has just had its drive
    /// plugged in is one backup, not two.
    fn queue(&self, rule: RuleId, trigger: RunTrigger) {
        if self.running() == Some(rule) {
            return;
        }
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.iter().any(|(queued, _)| *queued == rule) {
            return;
        }
        pending.push_back((rule, trigger));
    }

    /// Takes the next queued rule, if there is one.
    fn take_next(&self) -> Option<(RuleId, RunTrigger)> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
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
    spawn_run(app, rule, RunTrigger::Manual).map_err(|busy| {
        shelv_core::CoreError::Refused(if busy == rule {
            "that backup is already running".to_owned()
        } else {
            "another backup is running. Shelv runs one at a time so two rules \
             cannot write to one drive at once."
                .to_owned()
        })
    })
}

/// Asks for a scheduled run, which waits rather than being refused.
///
/// Called from the scheduler, which has nobody to tell that a run was
/// declined — so a rule it wants started joins the queue and begins when
/// the current run ends.
pub fn enqueue<R: Runtime>(app: &AppHandle<R>, rule: RuleId, trigger: RunTrigger) {
    let state = app.state::<AppState>();
    if spawn_run(app, rule, trigger).is_err() {
        state.runner.queue(rule, trigger);
    }
}

/// Starts a run now, or reports which rule is in the way.
fn spawn_run<R: Runtime>(
    app: &AppHandle<R>,
    rule: RuleId,
    trigger: RunTrigger,
) -> Result<(), RuleId> {
    let cancel = app.state::<AppState>().runner.claim(rule)?;

    let handle = app.clone();
    let spawned = thread::Builder::new()
        .name("shelv-backup".to_owned())
        .spawn(move || {
            let observer = Reporter::new(&handle, rule, Arc::clone(&cancel));
            let state = handle.state::<AppState>();

            let outcome = state.run(rule, trigger, &observer);

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
            notify(&handle, &finished);

            if let Err(e) = handle.emit(RUN_FINISHED, &finished) {
                tracing::warn!(error = %e, "could not tell the window the run finished");
            }

            // Whatever happened to this one, the queue moves on. A failed
            // run must not strand the rules behind it.
            if let Some((next, trigger)) = handle.state::<AppState>().runner.take_next() {
                let _ = spawn_run(&handle, next, trigger);
            }
        });

    if spawned.is_err() {
        // The claim has to be given back, or a failure to spawn leaves
        // Backup Now refusing every rule until the app restarts.
        app_state_release(app);
        tracing::error!("could not start the backup thread");
    }
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

/// Tells the user about a run that did not simply work.
///
/// **Only when something needs them.** A notification after every
/// successful backup is a notification nobody reads, and the whole value of
/// the refusal notice is that it interrupts. Success is in the window, in
/// the row, where somebody who wants to check can look.
fn notify<R: Runtime>(app: &AppHandle<R>, finished: &RunFinished) {
    use shelv_core::model::RunResult;
    use tauri_plugin_notification::NotificationExt;

    let (title, body) = if let Some(error) = &finished.error {
        ("Backup could not run", error.clone())
    } else {
        let worst = finished
            .summaries
            .iter()
            .filter_map(|summary| summary.result)
            .min_by_key(|result| match result {
                // Worst first, so one destination going wrong is what the
                // notification is about even if another went fine.
                RunResult::Refused => 0,
                RunResult::Failed => 1,
                RunResult::Partial => 2,
                RunResult::Cancelled => 3,
                RunResult::Ok => 4,
            });

        match worst {
            Some(RunResult::Refused) => (
                "Backup stopped itself",
                finished
                    .summaries
                    .iter()
                    .find_map(|summary| summary.note.clone())
                    .unwrap_or_else(|| {
                        "The backup would have removed most of the destination.".to_owned()
                    }),
            ),
            Some(RunResult::Failed) => (
                "Backup failed",
                "Shelv could not complete this backup. Open Shelv to see why.".to_owned(),
            ),
            Some(RunResult::Partial) => (
                "Backup finished with problems",
                "Some files could not be read or written. Open Shelv to see which.".to_owned(),
            ),
            // Cancelled is the user's own doing, and Ok needs no telling.
            _ => return,
        }
    };

    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!(error = %e, "could not show a notification");
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;

    fn rule(id: i64) -> RuleId {
        RuleId(id)
    }

    #[test]
    fn a_rule_is_queued_once_however_many_times_it_is_asked_for() {
        // A rule that is due *and* has just had its drive plugged in is one
        // backup, not two — and a scheduler sweeping every quarter of an
        // hour must not pile up copies of a rule that is waiting.
        let runner = Runner::default();
        runner.queue(rule(1), RunTrigger::Schedule);
        runner.queue(rule(1), RunTrigger::OnConnect);
        runner.queue(rule(2), RunTrigger::Schedule);

        assert_eq!(runner.take_next(), Some((rule(1), RunTrigger::Schedule)));
        assert_eq!(runner.take_next(), Some((rule(2), RunTrigger::Schedule)));
        assert_eq!(runner.take_next(), None);
    }

    #[test]
    fn the_rule_already_running_is_not_queued_behind_itself() {
        let runner = Runner::default();
        runner.claim(rule(1)).expect("the runner is idle");

        runner.queue(rule(1), RunTrigger::Schedule);
        assert_eq!(runner.take_next(), None, "it is already happening");

        runner.queue(rule(2), RunTrigger::Schedule);
        assert_eq!(runner.take_next(), Some((rule(2), RunTrigger::Schedule)));
    }

    #[test]
    fn claiming_reports_which_rule_is_in_the_way() {
        // Backup Now turns this into a message; the scheduler turns it into
        // a place in the queue. Both need to know it was refused and why.
        let runner = Runner::default();
        runner.claim(rule(1)).expect("the runner is idle");
        assert_eq!(runner.claim(rule(2)).err(), Some(rule(1)));
        assert_eq!(
            runner.claim(rule(1)).err(),
            Some(rule(1)),
            "including itself"
        );

        runner.release();
        assert!(runner.claim(rule(2)).is_ok(), "released, so free again");
    }
}

//! The thread that notices a rule is due.
//!
//! All the judgement lives in [`shelv_core::scheduler`], which is a pure
//! function over the store and the attached volumes. What is left here is
//! the plumbing: when to look, and what to do with what it finds.
//!
//! Two occasions to look, and they are not the same question:
//!
//! * **Every quarter of an hour.** Schedules are measured in calendar days
//!   at the finest, so a tick this slow is still far finer than any period
//!   — and being slow is what makes the other occasion worth having.
//! * **When a drive appears.** For an occasionally-connected drive this is
//!   the trigger that matters (`docs/PLAN.md` §1.1d): the drive is here
//!   now, and may not be when the period comes round.
//!
//! Anything that changes what is due — a rule created or edited — nudges
//! the scheduler rather than waiting out the tick, so a rule saved at 3pm
//! does not appear to do nothing until 3:15.

use std::thread;
use std::time::Duration;

use shelv_core::platform;
use shelv_core::scheduler::Sweep;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands::{now, AppState};
use crate::runner;

/// How often the periodic sweep runs.
///
/// Fifteen minutes. A daily rule cannot care about a quarter of an hour,
/// and a slow tick keeps a laptop's disk asleep; what would otherwise be
/// lost — a rule starting promptly — is covered by sweeping whenever
/// something actually changes.
pub const TICK: Duration = Duration::from_secs(15 * 60);

/// Starts the scheduler, for the lifetime of the application.
pub fn spawn<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    thread::Builder::new()
        .name("shelv-scheduler".to_owned())
        .spawn(move || loop {
            // On the first pass too: a machine that was off overnight has
            // work waiting, and making someone stare at the window for a
            // quarter of an hour to find that out would be absurd.
            look(&app, Sweep::Tick);
            thread::sleep(TICK);
        })
        // The app is still usable without it — every rule can be run by
        // hand — so this is logged rather than made fatal.
        .map_or_else(
            |e| tracing::error!(error = %e, "could not start the scheduler"),
            |_| tracing::debug!("scheduler started"),
        );
}

/// Looks now, because something changed that might have made a rule due.
pub fn nudge<R: Runtime>(app: &AppHandle<R>, reason: Sweep) {
    look(app, reason);
}

/// One sweep, queueing whatever it finds.
fn look<R: Runtime>(app: &AppHandle<R>, reason: Sweep) {
    let state = app.state::<AppState>();

    // Paused stops runs from starting, and stops them being queued to start
    // the moment it is lifted: somebody who paused for the afternoon does
    // not want four hours of backups at five o'clock.
    if state.paused() {
        tracing::debug!("skipping a sweep: backups are paused");
        return;
    }

    let zone = platform::host_local_time();

    let ready = match state.sweep(zone.as_ref(), now(), reason) {
        Ok(ready) => ready,
        Err(e) => {
            // Enumerating volumes fails transiently — a drive pulled out
            // mid-scan is the ordinary case. Try again next tick rather
            // than killing the scheduler.
            tracing::warn!(error = %e, "a scheduler sweep failed");
            return;
        }
    };

    for entry in ready {
        tracing::info!(
            rule = %entry.rule,
            trigger = ?entry.trigger,
            "queueing a scheduled backup"
        );
        runner::enqueue(app, entry.rule, entry.trigger);
    }
}

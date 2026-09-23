//! The background thread that keeps drive availability current.
//!
//! All the judgement lives in [`shelv_core::watch`], including why this
//! polls rather than listening for device events. What is left here is the
//! plumbing: a thread, a tick, and an event to the window when — and only
//! when — something actually changed.

use std::thread;

use std::collections::HashSet;

use shelv_core::model::VolumeId;
use shelv_core::scheduler::Sweep;
use shelv_core::view::VolumeStatus;
use shelv_core::watch::{VolumeWatch, POLL_INTERVAL};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::commands::{now, AppState};

/// The event the frontend listens for. Carries no payload: the frontend
/// re-reads the rules anyway, because availability is only half of what a
/// row shows, and a payload would be a second source of truth to keep in
/// step with the commands.
pub const VOLUMES_CHANGED: &str = "shelv://volumes-changed";

/// Starts watching, for the lifetime of the application.
///
/// Detached deliberately. There is nothing to join: the thread holds no
/// resource that needs releasing, and the process exiting is the only thing
/// that ends it.
pub fn spawn<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    thread::Builder::new()
        .name("shelv-volume-watch".to_owned())
        .spawn(move || {
            let mut watch = VolumeWatch::new();
            let mut available: HashSet<VolumeId> = HashSet::new();
            loop {
                tick(&app, &mut watch, &mut available);
                thread::sleep(POLL_INTERVAL);
            }
        })
        // A failure to spawn means the OS is out of threads. The app is
        // still usable — the table just will not update by itself — so this
        // is logged rather than made fatal.
        .map_or_else(
            |e| tracing::error!(error = %e, "could not start the volume watcher"),
            |_| tracing::debug!("volume watcher started"),
        );
}

fn tick<R: Runtime>(
    app: &AppHandle<R>,
    watch: &mut VolumeWatch,
    available: &mut HashSet<VolumeId>,
) {
    let poll = match app.state::<AppState>().poll_volumes(watch, now()) {
        Ok(poll) => poll,
        Err(e) => {
            // Enumerating volumes can fail transiently — a drive pulled out
            // mid-scan is the ordinary case. Log it and try again next tick
            // rather than killing the watcher.
            tracing::warn!(error = %e, "a volume poll failed");
            return;
        }
    };

    if !poll.changed {
        return;
    }

    tracing::debug!(volumes = poll.statuses.len(), "drive availability changed");
    if let Err(e) = app.emit(VOLUMES_CHANGED, ()) {
        tracing::warn!(error = %e, "could not notify the window");
    }

    // A drive *appearing* is a scheduling event; a drive being pulled out
    // is not. Comparing against the previous set rather than reacting to
    // any change is what keeps an unplug — or a rename, which also counts
    // as a change — from queueing a backup.
    let now_available = writable(&poll.statuses);
    let appeared = now_available.difference(available).count() > 0;
    *available = now_available;

    if appeared {
        crate::scheduler::nudge(app, Sweep::DriveAppeared);
    }
}

/// The volumes that can be written to right now.
fn writable(statuses: &[VolumeStatus]) -> HashSet<VolumeId> {
    statuses
        .iter()
        .filter(|status| status.availability.is_writable())
        .map(|status| status.volume.id)
        .collect()
}

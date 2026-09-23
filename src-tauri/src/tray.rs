//! The tray icon, and the pause switch behind it.
//!
//! Shelv runs as a **per-user tray process, never a Windows service**. That
//! is a correctness requirement before it is a convenience one: a service
//! context gets `STATUS_CLOUD_FILE_ACCESS_DENIED` instead of `OneDrive`
//! hydration (`docs/PLAN.md` §2), so the background half of a backup tool
//! that syncs cloud folders cannot be a service at all.
//!
//! Closing the window therefore hides it rather than quitting. A backup tool
//! that stops backing up because somebody closed a window is not automated,
//! and the tray is where it says so.

use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands::AppState;

/// Emitted when the pause switch moves, so a window that did not do it
/// still shows the truth.
pub const PAUSED_CHANGED: &str = "shelv://paused-changed";

/// Menu item ids. Strings because that is what the menu event carries.
const SHOW: &str = "show";
const PAUSE: &str = "pause";
const QUIT: &str = "quit";

/// Builds the tray icon and its menu.
///
/// Failing to build one is logged rather than fatal. A machine with no
/// system tray — a bare window manager, a locked-down desktop — still has a
/// working backup tool; it just has to be closed from the window.
pub fn build<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = try_build(app) {
        tracing::warn!(error = %e, "could not create the tray icon");
    }
}

fn try_build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = menu(app, false)?;
    let mut tray = TrayIconBuilder::with_id("shelv")
        .menu(&menu)
        .tooltip("Shelv — backup manager")
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event);

    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }

    tray.build(app)?;
    Ok(())
}

/// The menu, with the pause item worded for what pressing it will do.
fn menu<R: Runtime>(app: &AppHandle<R>, paused: bool) -> tauri::Result<Menu<R>> {
    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, SHOW, "Show Shelv", true, None::<&str>)?,
            &MenuItem::with_id(
                app,
                PAUSE,
                if paused {
                    "Resume backups"
                } else {
                    "Pause all backups"
                },
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, QUIT, "Quit Shelv", true, None::<&str>)?,
        ],
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri's menu-event callback hands the event over by value"
)]
fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        SHOW => show(app),
        PAUSE => {
            let paused = !app.state::<AppState>().paused();
            set_paused(app, paused);
        }
        // The only way out. Closing the window hides it, so without this
        // there would be no way to stop Shelv short of the task manager.
        QUIT => app.exit(0),
        other => tracing::debug!(id = other, "an unknown tray menu item was pressed"),
    }
}

/// Brings the window back, creating it if it was never opened.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) else {
        tracing::warn!("the main window is gone");
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Stops or resumes automatic backups.
///
/// **Pausing stops runs from starting; it does not stop one that is
/// going.** A copy that is half done is better finished than abandoned —
/// nothing is left half-written either way, but the next run would only
/// have to do it again — and Cancel is there for somebody who means the
/// stronger thing.
pub fn set_paused<R: Runtime>(app: &AppHandle<R>, paused: bool) {
    app.state::<AppState>().set_paused(paused);
    tracing::info!(paused, "automatic backups");

    // The tray menu carries the state as a word, so it has to be rebuilt.
    if let Some(tray) = app.tray_by_id("shelv") {
        match menu(app, paused) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(e) => tracing::warn!(error = %e, "could not update the tray menu"),
        }
    }

    if let Err(e) = tauri::Emitter::emit(app, PAUSED_CHANGED, paused) {
        tracing::warn!(error = %e, "could not tell the window about the pause");
    }
}

/// Turns running at login on or off.
///
/// Off by default: a backup tool that adds itself to startup without asking
/// has made a decision that is not its to make. The frontend asks through a
/// command that takes a boolean, so the plugin's own commands stay out of
/// `capabilities/` and the frontend gains no new reach.
pub fn set_run_at_login<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> shelv_core::Result<()> {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| shelv_core::CoreError::Io(format!("could not change run at login: {e}")))
}

/// Whether Shelv is set to run at login.
pub fn runs_at_login<R: Runtime>(app: &AppHandle<R>) -> bool {
    use tauri_plugin_autostart::ManagerExt;

    app.autolaunch().is_enabled().unwrap_or(false)
}

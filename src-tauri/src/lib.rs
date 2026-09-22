//! Tauri shell for Shelv.
//!
//! This crate owns the window, the IPC surface and nothing else. All backup
//! logic lives in `shelv-core`, which has no GUI dependency — see
//! `docs/PLAN.md` §2.2.

mod commands;
mod runner;
mod watcher;

use std::path::Path;

use shelv_core::platform;
use shelv_core::store::Store;
use tauri::{Runtime, WebviewUrl, WebviewWindowBuilder};

#[cfg(test)]
mod security_tests;

/// The label of the one window Shelv opens.
///
/// **Load-bearing.** `capabilities/default.json` scopes its permissions to
/// `"windows": ["main"]`, so a window under any other label holds none of
/// them. What that costs is `core:default`, and inside it `core:event` —
/// which is what `listen` needs. The drives pane and the rule table follow
/// attached drives by listening for `shelv://volumes-changed`, so they would
/// simply stop updating: no error, no crash, just a table quietly going
/// stale while a drive is plugged in and out.
///
/// Note it is *not* the folder picker that would break. That runs
/// `dialog()` from Rust inside `pick_folder`, and the ACL gates commands
/// invoked from the webview, not plugin calls the backend makes itself.
///
/// Nothing fails loudly if this drifts, so `security_tests` compares it
/// against the manifest.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Fatal startup failures.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// The Tauri runtime could not be built or run.
    #[error("could not start the application: {0}")]
    Runtime(#[from] tauri::Error),

    /// The rule database could not be opened or migrated. Starting without it
    /// would present an empty rule list, which reads as "your backups are
    /// gone"; refusing to start is the honest failure.
    #[error("could not open the Shelv database: {0}")]
    Store(#[from] shelv_core::CoreError),
}

/// Builds and runs the desktop application, returning once the last window
/// closes.
pub fn run() -> Result<(), StartupError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("SHELV_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let fs = platform::host_fs();
    let data_dir = fs.data_dir()?;
    let db_path = Store::default_path(fs.as_ref())?;
    tracing::info!(path = %db_path.display(), "opening the rule database");
    let store = Store::open(&db_path)?;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(commands::AppState::new(store, fs, db_path))
        .setup(move |app| {
            // The window is built here rather than declared in
            // tauri.conf.json because only this path can choose where the
            // webview keeps its profile. See `open_main_window`.
            open_main_window(app, &data_dir)?;

            // Started after the window so it has something to notify.
            watcher::spawn(app.handle());
            Ok(())
        })
        .invoke_handler(commands::handlers())
        .run(tauri::generate_context!())?;

    Ok(())
}

/// Opens Shelv's window, with its webview profile beside the database.
///
/// This exists in Rust rather than in `tauri.conf.json` for one reason: the
/// webview's data directory. Left alone, Tauri files it under the
/// reverse-DNS identifier — `%LOCALAPPDATA%\nz.eltam.shelv` — while the
/// database sits in `%LOCALAPPDATA%\Shelv`, so Shelv's data ends up in two
/// differently named folders and anyone clearing it out finds one of them.
/// A `dataDirectory` in the config cannot fix it either: that path is
/// resolved relative to `<local data>/<window label>`, and an absolute one
/// is discarded.
///
/// Pointing it at the directory the database was just opened from puts
/// everything under one product-named folder. The webview keeps to its own
/// `EBWebView` subdirectory, which it creates itself, so the two do not
/// become entangled.
fn open_main_window<R: Runtime>(app: &tauri::App<R>, data_dir: &Path) -> Result<(), tauri::Error> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::default())
        .title("Shelv")
        .inner_size(1480.0, 820.0)
        .min_inner_size(900.0, 480.0)
        .resizable(true)
        .theme(Some(tauri::Theme::Dark))
        // Painted before the frontend has rendered anything. Without it the
        // window flashes white on open, which on a dark theme is a jolt.
        .background_color(tauri::window::Color(0x14, 0x16, 0x1a, 0xff))
        .data_directory(data_dir.to_path_buf())
        .build()?;
    Ok(())
}

/// Where the webview keeps its profile, given Shelv's data directory.
///
/// The two runtimes do not agree on this, so it is not a guess to be made
/// once and applied everywhere. `WebView2` on Windows creates a single
/// `EBWebView` directory inside whatever user-data folder it is handed.
/// `WebKitGTK` writes its files — `CacheStorage`, `WebKitCache`,
/// `hsts-storage.sqlite`, `storage` and more — straight into that folder
/// alongside the database, with no subdirectory of its own.
///
/// Reporting `EBWebView` on both would name a path that does not exist on
/// Linux, which is worse than saying less: the whole point of the About
/// panel is to be somewhere a path can be trusted.
#[must_use]
pub fn webview_profile_dir(data_dir: &Path) -> std::path::PathBuf {
    if cfg!(windows) {
        data_dir.join("EBWebView")
    } else {
        data_dir.to_path_buf()
    }
}

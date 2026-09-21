//! Tauri shell for Shelv.
//!
//! This crate owns the window, the IPC surface and nothing else. All backup
//! logic lives in `shelv-core`, which has no GUI dependency — see
//! `docs/PLAN.md` §2.2.

mod commands;
mod watcher;

use shelv_core::platform;
use shelv_core::store::Store;

#[cfg(test)]
mod security_tests;

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
    let db_path = Store::default_path(fs.as_ref())?;
    tracing::info!(path = %db_path.display(), "opening the rule database");
    let store = Store::open(&db_path)?;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(commands::AppState::new(store, fs))
        .setup(|app| {
            // Started here rather than before the builder so it can reach
            // the managed state and the window it notifies.
            watcher::spawn(app.handle());
            Ok(())
        })
        .invoke_handler(commands::handlers())
        .run(tauri::generate_context!())?;

    Ok(())
}

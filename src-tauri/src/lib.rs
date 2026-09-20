//! Tauri shell for Shelv.
//!
//! This crate owns the window, the IPC surface and nothing else. All backup
//! logic lives in `shelv-core`, which has no GUI dependency — see
//! `docs/PLAN.md` §2.2.

mod commands;

#[cfg(test)]
mod security_tests;

/// Fatal startup failures.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// The Tauri runtime could not be built or run.
    #[error("could not start the application: {0}")]
    Runtime(#[from] tauri::Error),
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

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(commands::handlers())
        .run(tauri::generate_context!())?;

    Ok(())
}

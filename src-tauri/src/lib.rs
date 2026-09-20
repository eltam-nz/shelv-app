//! Tauri shell for Shelv.
//!
//! This crate owns the window, the IPC surface and nothing else. All backup
//! logic lives in `shelv-core`, which has no GUI dependency — see
//! `docs/PLAN.md` §2.2.

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

mod commands;

/// Builds and runs the desktop application.
pub fn run() {
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
        .run(tauri::generate_context!())
        .expect("error while running Shelv");
}

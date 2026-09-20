//! Desktop entrypoint. Thin on purpose: everything lives in the library so it
//! can be linked by tests.

// Hides the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match shelv_lib::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Logging may not be initialised yet if the failure was early, so
            // write to stderr directly as well.
            eprintln!("shelv: {e}");
            tracing::error!(error = %e, "startup failed");
            ExitCode::FAILURE
        }
    }
}

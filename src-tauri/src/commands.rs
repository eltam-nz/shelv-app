//! The IPC surface.
//!
//! Commands take rule, destination and tag **ids** — never paths. Paths are
//! resolved inside `shelv-core` from the database, so a compromised WebView
//! cannot invent one (`docs/PLAN.md` §4, T1). The real command set lands in
//! task #7; this is the wiring it plugs into.

use tauri::ipc::Invoke;

/// Reports the running version. Exists so the IPC path is exercised end to end
/// from the first commit rather than first being tested in M2.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The generated invoke handler for every command in this module.
pub fn handlers() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![app_version]
}

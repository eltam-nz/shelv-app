//! Tests that the frontend cannot reach the filesystem.
//!
//! The security model in `docs/PLAN.md` §4 rests on one property: a compromised
//! or malicious `WebView` can start a backup the user already configured, but
//! cannot invent a path. That holds only while no filesystem command is
//! reachable over IPC.
//!
//! `tauri-plugin-fs` *is* in the dependency graph — `tauri-plugin-dialog`
//! pulls it in so a picked path can be added to the fs scope — so being absent
//! from `Cargo.toml` is not the guarantee. Two independent checks are, and it
//! is worth being precise about which proves what:
//!
//! * **The runtime tests** drive real IPC through Tauri's mock runtime with
//!   `tauri-plugin-fs` *deliberately registered*, and show the commands are
//!   still refused. That proves the ACL denies by default: registering a
//!   plugin does not make it reachable without a capability granting it.
//!   It does **not** validate our own manifest, because `mock_context` embeds
//!   no capabilities at all — `assert_the_mock_grants_nothing` pins that fact
//!   so this test is not later mistaken for something stronger.
//!
//! * **The manifest tests** read `capabilities/default.json` and
//!   `tauri.conf.json` directly and check what the shipping app actually
//!   grants. This is where a regression in our own configuration is caught.
//!
//! There is a third layer underneath both, provided by Tauri rather than us:
//! the build script refuses to compile a capability naming a permission whose
//! plugin is not a dependency of this crate. Adding `fs:allow-read-text-file`
//! to the manifest fails the build outright. What that does *not* catch is a
//! permission that is valid but was never reviewed — `dialog:allow-save`, say,
//! which compiles — and that is precisely what `EXPECTED_PERMISSIONS` is for.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a panic in a test is the failure report"
)]

use serde_json::json;
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::WebviewWindowBuilder;

/// Reads a config file from the crate root at run time.
///
/// Deliberately not `include_str!`: that bakes the contents in at compile
/// time, so editing the JSON alone need not rebuild the test, and a weakened
/// manifest could pass against a stale copy of itself.
fn read_config(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// Filesystem and process commands that must never answer.
///
/// Spelled as the frontend would call them, `plugin:<name>|<command>`.
const FORBIDDEN_COMMANDS: &[&str] = &[
    "plugin:fs|read_file",
    "plugin:fs|read_text_file",
    "plugin:fs|write_file",
    "plugin:fs|write_text_file",
    "plugin:fs|remove",
    "plugin:fs|rename",
    "plugin:fs|copy_file",
    "plugin:fs|mkdir",
    "plugin:fs|read_dir",
    "plugin:fs|exists",
    "plugin:fs|stat",
    "plugin:shell|execute",
    "plugin:shell|open",
    "plugin:http|fetch",
    "plugin:process|exit",
];

/// The complete set of permissions the app is allowed to grant.
///
/// Adding one is a deliberate act, so it has to be added here too.
const EXPECTED_PERMISSIONS: &[&str] =
    &["core:default", "dialog:allow-open", "notification:default"];

fn build_app() -> tauri::App<tauri::test::MockRuntime> {
    let store = shelv_core::store::Store::open_in_memory().expect("an in-memory store");
    mock_builder()
        .manage(crate::commands::AppState::new(
            store,
            shelv_core::platform::host_fs(),
            // Never opened: these tests exercise the IPC surface, not a
            // backup. A run would open this path for itself.
            std::path::PathBuf::from("shelv-tests.db"),
        ))
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(crate::commands::handlers())
        .build(mock_context(noop_assets()))
        .expect("the app should build under the mock runtime")
}

fn request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.to_owned(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .expect("a valid IPC origin"),
        body: tauri::ipc::InvokeBody::Json(body),
        headers: tauri::http::HeaderMap::new(),
        invoke_key: INVOKE_KEY.to_owned(),
    }
}

#[test]
fn no_filesystem_command_answers_over_ipc() {
    // `build_app` registers tauri-plugin-fs on purpose. If registration alone
    // were enough to expose these commands, this test would fail.
    let app = build_app();
    let webview = WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
        .build()
        .expect("the main window should build");

    for cmd in FORBIDDEN_COMMANDS {
        let response = get_ipc_response(
            &webview,
            request(cmd, json!({ "path": "/etc/passwd", "contents": "x" })),
        );
        assert!(
            response.is_err(),
            "{cmd} answered over IPC even though no capability grants it. The \
             frontend must have no filesystem access at all (docs/PLAN.md §4, \
             T1): commands take rule ids, and paths are resolved in shelv-core."
        );
    }
}

/// Records that the mock runtime embeds no capabilities.
///
/// `notification:default` *is* granted by `capabilities/default.json`, yet it
/// is refused here. That is the proof that the runtime tests exercise Tauri's
/// deny-by-default behaviour rather than our manifest — and a tripwire if a
/// future Tauri version starts loading real capabilities into the mock, at
/// which point these tests get stronger and this one should be deleted.
#[test]
fn assert_the_mock_grants_nothing() {
    let app = build_app();
    let webview = WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
        .build()
        .expect("the main window should build");

    let response = get_ipc_response(
        &webview,
        request("plugin:notification|is_permission_granted", json!({})),
    );
    assert!(
        response.is_err(),
        "the mock runtime now honours real capabilities — the runtime tests \
         above can be strengthened to check the shipping manifest directly, \
         and this test removed"
    );
}

#[test]
fn the_probe_would_notice_a_command_that_does_answer() {
    // Guards the test above: if `get_ipc_response` started returning Err for
    // everything, the loop would pass while proving nothing.
    let app = build_app();
    let webview = WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
        .build()
        .expect("the main window should build");

    let response = get_ipc_response(&webview, request("app_version", json!({})));
    assert!(
        response.is_ok(),
        "a registered command should answer, otherwise the forbidden-command \
         test proves nothing"
    );
}

#[test]
fn the_capability_manifest_grants_only_what_is_expected() {
    let manifest = read_config("capabilities/default.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest).expect("capabilities/default.json should be valid JSON");

    let granted: Vec<&str> = parsed
        .get("permissions")
        .and_then(serde_json::Value::as_array)
        .expect("the manifest should list permissions")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();

    for permission in &granted {
        assert!(
            EXPECTED_PERMISSIONS.contains(permission),
            "the capability manifest grants '{permission}', which is not in the \
             reviewed set. If this is intended, add it to EXPECTED_PERMISSIONS \
             and say why in docs/PLAN.md §4.2."
        );
        assert!(
            !permission.starts_with("fs:") && !permission.starts_with("shell:"),
            "'{permission}' would give the frontend filesystem or process access"
        );
    }
}

#[test]
fn the_content_security_policy_is_restrictive() {
    let config = read_config("tauri.conf.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&config).expect("tauri.conf.json should be valid JSON");

    let security = parsed
        .pointer("/app/security")
        .expect("the config should have a security section");

    let csp = security
        .get("csp")
        .and_then(serde_json::Value::as_str)
        .expect("a production CSP should be set");

    // Inline and remote script are the two ways a crafted filename rendered in
    // the rule table could turn into code execution (docs/PLAN.md §4, T1).
    assert!(csp.contains("default-src 'self'"), "CSP: {csp}");
    assert!(csp.contains("script-src 'self'"), "CSP: {csp}");
    assert!(
        !csp.contains("script-src 'self' 'unsafe-inline'"),
        "the production CSP must not allow inline script: {csp}"
    );
    assert!(!csp.contains("unsafe-eval"), "CSP: {csp}");
    assert!(csp.contains("object-src 'none'"), "CSP: {csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "CSP: {csp}");

    assert_eq!(
        parsed.pointer("/app/withGlobalTauri"),
        Some(&serde_json::Value::Bool(false)),
        "withGlobalTauri exposes the IPC bridge on `window`, widening what \
         injected script can reach"
    );
    assert_eq!(
        security.get("freezePrototype"),
        Some(&serde_json::Value::Bool(true)),
        "prototype pollution is a standard step in escalating an XSS"
    );
}

#[test]
fn the_app_makes_no_network_requests_of_its_own() {
    // PLAN §4, T7: the only outbound request Shelv ever makes is the update
    // check, which is not wired up yet. connect-src must therefore allow the
    // IPC origin and nothing else on the public internet.
    let config = read_config("tauri.conf.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&config).expect("tauri.conf.json should be valid JSON");
    let csp = parsed
        .pointer("/app/security/csp")
        .and_then(serde_json::Value::as_str)
        .expect("a production CSP should be set");

    let connect_src = csp
        .split(';')
        .map(str::trim)
        .find(|d| d.starts_with("connect-src"))
        .expect("connect-src should be set explicitly");

    assert!(
        !connect_src.contains("https://") && !connect_src.contains("http://tauri"),
        "connect-src should not reach the internet: {connect_src}"
    );
    assert!(connect_src.contains("'self'"), "{connect_src}");
}

#[test]
fn the_main_window_is_the_one_the_capability_applies_to() {
    // A capability is scoped by window label. A window built under a different
    // label would silently get no permissions — or, worse, a future capability
    // with a wildcard would apply where it was not reviewed.
    let manifest = read_config("capabilities/default.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest).expect("capabilities/default.json should be valid JSON");

    let windows: Vec<&str> = parsed
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .expect("the capability should name its windows")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();

    assert_eq!(windows, vec!["main"], "only the main window is reviewed");

    // The label the app actually builds under has to be one of those, or the
    // window holds no permissions at all. That used to be asserted by
    // building a window called "main" and checking it succeeded, which
    // proved nothing: building a window under any label succeeds. Since the
    // window moved into Rust the label is a constant, so compare it.
    assert!(
        windows.contains(&crate::MAIN_WINDOW_LABEL),
        "the window Shelv opens is labelled '{}', which the capability does \
         not cover: it would start with none of its permissions, losing \
         core:event and with it the live drive updates — no error, just a \
         table quietly going stale",
        crate::MAIN_WINDOW_LABEL,
    );
}

#[test]
fn the_webview_profile_sits_beside_the_database() {
    // Both under one product-named folder, rather than the database under
    // the product name and the webview under the reverse-DNS identifier,
    // which is what Tauri does when left alone. Two differently named
    // folders means anyone clearing Shelv's data finds one of them.
    let fs = shelv_core::platform::host_fs();
    let data_dir = fs.data_dir().expect("a data directory");
    let database = shelv_core::store::Store::default_path(fs.as_ref()).expect("a database path");
    let profile = crate::webview_profile_dir(&data_dir);

    // Asserted as containment rather than as a shared parent, because the
    // two webview runtimes disagree about the shape: WebView2 makes itself
    // an `EBWebView` subdirectory, WebKitGTK writes straight into the folder
    // it is given. What has to hold on both is that one folder holds
    // everything.
    assert_eq!(
        database.parent(),
        Some(data_dir.as_path()),
        "the database should sit directly in the data directory"
    );
    assert!(
        profile.starts_with(&data_dir),
        "the webview profile ({}) should be at or inside {}",
        profile.display(),
        data_dir.display()
    );
}

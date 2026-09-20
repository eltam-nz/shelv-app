//! The IPC surface.
//!
//! Every command takes **ids**, never paths. Paths are resolved inside
//! `shelv-core` from the database, so a compromised `WebView` can drive the
//! backups the user already configured but cannot invent a location
//! (`docs/PLAN.md` §4, T1). `security_tests` proves the frontend has no
//! filesystem capability of its own.
//!
//! Commands are synchronous because each is a short database read or write.
//! The engine's long-running work does not go through here: M1 runs it on a
//! background task and reports progress by event, so a backup never blocks
//! the UI thread.

#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri's command macro requires State and owned argument types; \
              State is a cheap reference wrapper"
)]

use std::sync::Mutex;

use shelv_core::model::{Rule, RuleId, RuleSpec, Run, Tag, TagId, VolumePath};
use shelv_core::platform::PlatformFs;
use shelv_core::store::Store;
use shelv_core::view::{rule_rows, volume_statuses, RuleRow, VolumeStatus};
use shelv_core::{CoreError, Result};
use tauri::ipc::Invoke;
use tauri::{Runtime, State};

/// Everything the commands need, owned by the Tauri app.
pub struct AppState {
    store: Mutex<Store>,
    fs: Box<dyn PlatformFs>,
}

impl AppState {
    /// Builds the state around an already-open store.
    #[must_use]
    pub fn new(store: Store, fs: Box<dyn PlatformFs>) -> Self {
        Self {
            store: Mutex::new(store),
            fs,
        }
    }

    /// Runs `f` against the store.
    ///
    /// A poisoned mutex means an earlier command panicked mid-write. The
    /// database itself is still consistent, because every multi-statement
    /// write goes through a transaction, so this reports the problem rather
    /// than propagating the panic.
    fn with_store<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let guard = self.store.lock().map_err(|_| {
            CoreError::Store(
                "the database lock was poisoned by an earlier failure; restart Shelv".into(),
            )
        })?;
        f(&guard)
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

/// The running version, for the About view and for support reports.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Every rule, with its tags, destinations, availability and last result.
///
/// This is what the rule table renders.
#[tauri::command]
fn list_rules(state: State<'_, AppState>) -> Result<Vec<RuleRow>> {
    state.with_store(|store| rule_rows(store, state.fs.as_ref()))
}

/// One rule, in the same shape as a table row.
#[tauri::command]
fn get_rule(state: State<'_, AppState>, id: RuleId) -> Result<RuleRow> {
    state.with_store(|store| {
        rule_rows(store, state.fs.as_ref())?
            .into_iter()
            .find(|row| row.rule.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("rule {id}")))
    })
}

/// Creates a rule and returns its id.
#[tauri::command]
fn create_rule(state: State<'_, AppState>, spec: RuleSpec) -> Result<RuleId> {
    state.with_store(|store| store.create_rule(&spec, now()))
}

/// Replaces a rule's configuration.
#[tauri::command]
fn update_rule(state: State<'_, AppState>, id: RuleId, spec: RuleSpec) -> Result<()> {
    state.with_store(|store| store.update_rule(id, &spec))
}

/// Deletes a rule, along with its destinations, tag links and run history.
///
/// This removes Shelv's record of the backup. It does not touch any file that
/// has already been backed up.
#[tauri::command]
fn delete_rule(state: State<'_, AppState>, id: RuleId) -> Result<()> {
    state.with_store(|store| store.delete_rule(id))
}

/// Adds a destination to a rule.
#[tauri::command]
fn add_destination(
    state: State<'_, AppState>,
    rule: RuleId,
    path: VolumePath,
    sort_order: i64,
) -> Result<shelv_core::model::DestinationId> {
    state.with_store(|store| store.add_destination(rule, &path, sort_order))
}

/// Removes a destination from a rule.
#[tauri::command]
fn delete_destination(
    state: State<'_, AppState>,
    id: shelv_core::model::DestinationId,
) -> Result<()> {
    state.with_store(|store| store.delete_destination(id))
}

/// Every tag.
#[tauri::command]
fn list_tags(state: State<'_, AppState>) -> Result<Vec<Tag>> {
    state.with_store(Store::tags)
}

/// Creates a tag.
///
/// `colour` is a palette token, not a raw colour, so the theme can guarantee
/// it stays readable against the dark background.
#[tauri::command]
fn create_tag(state: State<'_, AppState>, name: String, colour: String) -> Result<TagId> {
    state.with_store(|store| store.create_tag(&name, &colour))
}

/// Deletes a tag and unlinks it from every rule.
#[tauri::command]
fn delete_tag(state: State<'_, AppState>, id: TagId) -> Result<()> {
    state.with_store(|store| store.delete_tag(id))
}

/// Replaces the set of tags on a rule.
#[tauri::command]
fn set_rule_tags(state: State<'_, AppState>, rule: RuleId, tags: Vec<TagId>) -> Result<()> {
    state.with_store(|store| store.set_rule_tags(rule, &tags))
}

/// Every known volume and whether it can be written to right now.
#[tauri::command]
fn list_volumes(state: State<'_, AppState>) -> Result<Vec<VolumeStatus>> {
    state.with_store(|store| volume_statuses(store, state.fs.as_ref()))
}

/// A rule's run history, newest first.
#[tauri::command]
fn list_runs(state: State<'_, AppState>, rule: RuleId, limit: u32) -> Result<Vec<Run>> {
    state.with_store(|store| store.runs_for_rule(rule, limit))
}

/// A rule's raw configuration, for the editor.
#[tauri::command]
fn get_rule_spec(state: State<'_, AppState>, id: RuleId) -> Result<Rule> {
    state.with_store(|store| store.rule(id))
}

/// Seconds since the Unix epoch.
///
/// A clock before 1970 yields 0 rather than panicking; a wrong timestamp is
/// a cosmetic problem, an aborted backup is not.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The generated invoke handler for every command in this module.
///
/// Generic over the runtime so the security tests drive the *same* handler
/// the app registers, rather than a rebuilt approximation of it.
pub fn handlers<R: Runtime>() -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        app_version,
        list_rules,
        get_rule,
        get_rule_spec,
        create_rule,
        update_rule,
        delete_rule,
        add_destination,
        delete_destination,
        list_tags,
        create_tag,
        delete_tag,
        set_rule_tags,
        list_volumes,
        list_runs,
    ]
}

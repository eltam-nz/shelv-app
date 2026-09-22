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
use shelv_core::safety::{check_rule, RuleProblem};
use shelv_core::store::Store;
use shelv_core::view::{
    drive_rows, rule_rows, volume_statuses, DataLocations, DriveRow, RuleRow, VolumeStatus,
};
use shelv_core::volumes::{resolve_picked_folder, PickedFolder};
use shelv_core::{CoreError, Result};
use tauri::ipc::Invoke;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_dialog::DialogExt;

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

    /// Runs one watcher tick against the store.
    ///
    /// Exposed so the background watcher can reuse the same lock discipline
    /// as the commands rather than opening a second connection: two writers
    /// to one `SQLite` file is a busy-timeout waiting to happen, and the
    /// watcher writes whenever a drive is renamed.
    pub fn poll_volumes(
        &self,
        watch: &mut shelv_core::watch::VolumeWatch,
        now: i64,
    ) -> Result<shelv_core::watch::Poll> {
        self.with_store(|store| watch.poll(store, self.fs.as_ref(), now))
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

/// Where Shelv keeps its database and the window's own preferences.
///
/// Derived from the same `data_dir` the window and the store were opened
/// from, rather than rebuilt from the product name, so the About panel
/// cannot end up naming a folder the app is not actually using.
#[tauri::command]
fn data_locations(state: State<'_, AppState>) -> Result<DataLocations> {
    let data_dir = state.fs.data_dir()?;
    Ok(DataLocations {
        database: Store::default_path(state.fs.as_ref())?,
        webview_profile: crate::webview_profile_dir(&data_dir),
    })
}

/// Opens Shelv's data folder in the system file manager.
///
/// Takes **no path**, and that is the point. Granting the frontend a
/// general "open this path" capability would let a compromised `WebView` ask
/// the operating system to launch anything it liked, which is precisely the
/// widening the id-only command surface exists to prevent (`docs/PLAN.md`
/// §4, T1). This opens one directory, chosen here.
#[tauri::command]
fn reveal_data_folder(state: State<'_, AppState>) -> Result<()> {
    let data_dir = state.fs.data_dir()?;

    // The folder may not exist yet on a first run that has not written
    // anything. Creating it is friendlier than opening a file manager on
    // nothing, and it is the same directory the store would create anyway.
    std::fs::create_dir_all(&data_dir)?;

    let program = if cfg!(windows) {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(&data_dir)
        .spawn()
        // Explorer exits non-zero in cases where it has still done the right
        // thing, so the child is deliberately not waited on: whether the
        // window appeared is the user's to see, not ours to adjudicate.
        .map_err(|e| CoreError::Io(format!("could not open {}: {e}", data_dir.display())))?;
    Ok(())
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

/// Opens the native folder dialog and resolves the choice against its volume.
///
/// This is the only way a location enters Shelv. The dialog is the operating
/// system's own consent step, and what comes back is a volume id plus a
/// relative path — never something the frontend could have invented
/// (`docs/PLAN.md` §4.2). Returns `None` if the user cancelled.
///
/// Registering the volume is a side effect of picking: until a folder is
/// chosen on a drive, Shelv has no record of it at all.
#[tauri::command]
async fn pick_folder<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<Option<PickedFolder>> {
    // Blocking is correct here: Tauri runs async commands off the main
    // thread, and the non-blocking form would deadlock the event loop.
    let Some(chosen) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };

    let path = chosen.into_path().map_err(|e| {
        CoreError::Invalid(format!("the chosen folder is not a filesystem path: {e}"))
    })?;

    state
        .with_store(|store| resolve_picked_folder(store, state.fs.as_ref(), &path, now()))
        .map(Some)
}

/// Checks a rule without saving it, so the editor can show problems as they
/// are introduced rather than only on save.
#[tauri::command]
fn validate_rule(
    state: State<'_, AppState>,
    spec: RuleSpec,
    destinations: Vec<VolumePath>,
) -> Result<Vec<RuleProblem>> {
    state.with_store(|store| Ok(validate(store, state.fs.as_ref(), &spec, &destinations)))
}

/// Creates a rule and its destinations, refusing one that would not be safe
/// to run.
///
/// Validation happens here as well as in the editor, because a rule is a
/// standing instruction: by the time the engine acts on it, whoever wrote it
/// is usually not watching.
#[tauri::command]
fn create_rule(
    state: State<'_, AppState>,
    spec: RuleSpec,
    destinations: Vec<VolumePath>,
    tags: Vec<TagId>,
) -> Result<RuleId> {
    state.with_store(|store| {
        refuse_if_unsafe(store, state.fs.as_ref(), &spec, &destinations)?;
        let id = store.create_rule(&spec, now())?;
        for (order, destination) in destinations.iter().enumerate() {
            store.add_destination(id, destination, i64::try_from(order).unwrap_or(i64::MAX))?;
        }
        store.set_rule_tags(id, &tags)?;
        Ok(id)
    })
}

/// Replaces a rule's configuration, destinations and tags.
#[tauri::command]
fn update_rule(
    state: State<'_, AppState>,
    id: RuleId,
    spec: RuleSpec,
    destinations: Vec<VolumePath>,
    tags: Vec<TagId>,
) -> Result<()> {
    state.with_store(|store| {
        refuse_if_unsafe(store, state.fs.as_ref(), &spec, &destinations)?;
        store.update_rule(id, &spec)?;

        // Replace the destination set. Existing rows are removed rather than
        // reconciled, since a destination carries no state worth preserving
        // — run history points at the rule, not at the destination row.
        for existing in store.destinations(id)? {
            store.delete_destination(existing.id)?;
        }
        for (order, destination) in destinations.iter().enumerate() {
            store.add_destination(id, destination, i64::try_from(order).unwrap_or(i64::MAX))?;
        }
        store.set_rule_tags(id, &tags)
    })
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

/// Every drive Shelv has recorded, with its status and how many rules use it.
///
/// This is what the drives pane renders. Only recorded drives: a drive is
/// added by picking a folder on it through `pick_folder`, which is the
/// operating system's own consent step, so there is no second way in and
/// nothing here enumerates the user's attached hardware.
#[tauri::command]
fn list_drives(state: State<'_, AppState>) -> Result<Vec<DriveRow>> {
    state.with_store(|store| drive_rows(store, state.fs.as_ref()))
}

/// Sets or clears the name the user gave a drive.
///
/// Display only. Nothing resolves or writes on a nickname — the volume
/// identity remains the only key anything is matched by, because a name that
/// decided where a backup went would reintroduce the wrong-drive failure
/// (`docs/PLAN.md` §4, T3).
#[tauri::command]
fn set_drive_nickname(
    state: State<'_, AppState>,
    id: shelv_core::model::VolumeId,
    nickname: Option<String>,
) -> Result<()> {
    state.with_store(|store| store.set_volume_nickname(id, nickname.as_deref()))
}

/// Removes Shelv's record of a drive.
///
/// Refused while any rule still points at it. Nothing on the drive itself is
/// touched, and picking a folder on it again re-registers it under the same
/// identity.
#[tauri::command]
fn forget_drive(state: State<'_, AppState>, id: shelv_core::model::VolumeId) -> Result<()> {
    state.with_store(|store| store.delete_volume(id))
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

/// Runs the safety guards against a rule, resolving each volume's current
/// status so the guards can refuse an unusable one.
fn validate(
    store: &Store,
    fs: &dyn PlatformFs,
    spec: &RuleSpec,
    destinations: &[VolumePath],
) -> Vec<RuleProblem> {
    let statuses = volume_statuses(store, fs).unwrap_or_default();
    let status_for = |id| statuses.iter().find(|s| s.volume.id == id);

    // Containment has to be compared the way the source filesystem does, or
    // "Pictures/Backups" inside "pictures" slips through on NTFS.
    let case_insensitive = store
        .volume(spec.source.volume)
        .ok()
        .and_then(|v| v.filesystem)
        .is_some_and(|fs_name| {
            ["ntfs", "exfat", "vfat", "fat32", "msdos"]
                .iter()
                .any(|f| fs_name.eq_ignore_ascii_case(f))
        });

    let destination_statuses: Vec<_> = destinations.iter().map(|d| status_for(d.volume)).collect();

    check_rule(
        spec,
        destinations,
        status_for(spec.source.volume),
        &destination_statuses,
        case_insensitive,
    )
}

/// Refuses to save a rule that the guards reject.
fn refuse_if_unsafe(
    store: &Store,
    fs: &dyn PlatformFs,
    spec: &RuleSpec,
    destinations: &[VolumePath],
) -> Result<()> {
    let problems = validate(store, fs, spec, destinations);
    if problems.is_empty() {
        return Ok(());
    }
    Err(CoreError::Refused(
        serde_json::to_string(&problems)
            .unwrap_or_else(|_| "this rule is not safe to run".to_owned()),
    ))
}

/// Seconds since the Unix epoch.
///
/// A clock before 1970 yields 0 rather than panicking; a wrong timestamp is
/// a cosmetic problem, an aborted backup is not.
pub fn now() -> i64 {
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
        data_locations,
        reveal_data_folder,
        pick_folder,
        validate_rule,
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
        list_drives,
        set_drive_nickname,
        forget_drive,
        list_runs,
    ]
}

//! Persistence.
//!
//! One `SQLite` database under the platform's data directory, holding rules,
//! tags, known volumes and run history. It contains paths and metadata only —
//! no credentials, because Shelv has none to store (`docs/PLAN.md` §4).

mod migrations;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::error::CoreError;
use crate::model::{
    Destination, DestinationId, EventLevel, Layout, Packaging, PlaceholderPolicy, Retention, Rule,
    RuleId, RuleSpec, Run, RunEvent, RunId, RunResult, RunStats, RunTrigger, Schedule,
    StoredVolume, Tag, TagId, VolumeId, VolumePath,
};
use crate::platform::{DriveType, VolumeIdentity, VolumeIdentityKind, VolumeInfo};
use crate::Result;

pub use migrations::LATEST_VERSION;

/// The database file's name within the data directory.
pub const DB_FILENAME: &str = "shelv.db";

/// Converts a rusqlite error into a store error.
fn store_err(context: &str) -> impl Fn(rusqlite::Error) -> CoreError + '_ {
    move |e| CoreError::Store(format!("{context}: {e}"))
}

/// Widens a database integer to `u64`, treating a negative as zero.
///
/// `SQLite` has no unsigned type, so a corrupt or hand-edited row could hold a
/// negative count. Clamping keeps the arithmetic sane without panicking.
fn as_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Narrows a `u64` for storage, saturating at `i64::MAX`.
fn as_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The rule database.
pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Opens, creating and migrating the database if needed.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(store_err("could not open the database"))?;
        Self::from_connection(conn)
    }

    /// Opens a private in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(store_err("could not open an in-memory database"))?;
        Self::from_connection(conn)
    }

    fn from_connection(mut conn: Connection) -> Result<Self> {
        // SQLite disables foreign keys by default, per connection. Without
        // this the ON DELETE CASCADE clauses in the schema do nothing and
        // deleting a rule orphans its destinations, runs and tag links.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(store_err("could not configure the database"))?;

        migrations::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// The default database location for this platform.
    pub fn default_path(fs: &dyn crate::platform::PlatformFs) -> Result<PathBuf> {
        Ok(fs.data_dir()?.join(DB_FILENAME))
    }

    /// The schema version currently applied.
    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(store_err("could not read the schema version"))
    }

    // -- volumes ----------------------------------------------------------

    /// Records a volume, or updates what is known about one already recorded.
    ///
    /// Matching is on identity, never on mount point, so a drive that comes
    /// back as a different letter is still recognised as the same volume.
    pub fn upsert_volume(&self, info: &VolumeInfo, last_seen_at: Option<i64>) -> Result<VolumeId> {
        let identity = &info.identity;
        let kind = serde_plain_kind(identity.kind);
        self.conn
            .execute(
                "INSERT INTO volume (identity_kind, identity, serial, label, filesystem,
                                     drive_type, is_sync_root, last_mount, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (identity_kind, identity) DO UPDATE SET
                     serial       = excluded.serial,
                     label        = excluded.label,
                     filesystem   = excluded.filesystem,
                     drive_type   = excluded.drive_type,
                     is_sync_root = excluded.is_sync_root,
                     last_mount   = excluded.last_mount,
                     last_seen_at = excluded.last_seen_at",
                rusqlite::params![
                    kind,
                    identity.value,
                    info.serial,
                    info.label,
                    info.filesystem,
                    drive_type_str(info.drive_type),
                    i64::from(info.is_sync_root),
                    path_to_db(&info.mount_point),
                    last_seen_at,
                ],
            )
            .map_err(store_err("could not record the volume"))?;

        self.conn
            .query_row(
                "SELECT id FROM volume WHERE identity_kind = ?1 AND identity = ?2",
                rusqlite::params![kind, identity.value],
                |row| row.get(0),
            )
            .map_err(store_err("could not read back the volume id"))
    }

    /// Every volume Shelv has recorded, attached or not.
    pub fn volumes(&self) -> Result<Vec<StoredVolume>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, identity_kind, identity, serial, label, filesystem, drive_type,
                        is_sync_root, last_seen_at, last_mount
                 FROM volume ORDER BY id",
            )
            .map_err(store_err("could not list volumes"))?;
        let rows = stmt
            .query_map([], read_volume)
            .map_err(store_err("could not list volumes"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a volume row"))
    }

    /// One volume by id.
    pub fn volume(&self, id: VolumeId) -> Result<StoredVolume> {
        self.conn
            .query_row(
                "SELECT id, identity_kind, identity, serial, label, filesystem, drive_type,
                        is_sync_root, last_seen_at, last_mount
                 FROM volume WHERE id = ?1",
                [id],
                read_volume,
            )
            .optional()
            .map_err(store_err("could not read the volume"))?
            .ok_or_else(|| CoreError::NotFound(format!("volume {id}")))
    }

    // -- tags -------------------------------------------------------------

    /// Creates a tag.
    pub fn create_tag(&self, name: &str, colour: &str) -> Result<TagId> {
        self.conn
            .execute(
                "INSERT INTO tag (name, colour) VALUES (?1, ?2)",
                rusqlite::params![name, colour],
            )
            .map_err(store_err("could not create the tag"))?;
        Ok(TagId(self.conn.last_insert_rowid()))
    }

    /// Every tag, ordered by name.
    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, colour FROM tag ORDER BY name")
            .map_err(store_err("could not list tags"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    colour: row.get(2)?,
                })
            })
            .map_err(store_err("could not list tags"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a tag row"))
    }

    /// Deletes a tag, and its links to any rules.
    pub fn delete_tag(&self, id: TagId) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM tag WHERE id = ?1", [id])
            .map_err(store_err("could not delete the tag"))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("tag {id}")));
        }
        Ok(())
    }

    /// Replaces the set of tags on a rule.
    pub fn set_rule_tags(&self, rule: RuleId, tags: &[TagId]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(store_err("could not begin a transaction"))?;
        tx.execute("DELETE FROM rule_tag WHERE rule_id = ?1", [rule])
            .map_err(store_err("could not clear the rule's tags"))?;
        for tag in tags {
            tx.execute(
                "INSERT OR IGNORE INTO rule_tag (rule_id, tag_id) VALUES (?1, ?2)",
                rusqlite::params![rule, tag],
            )
            .map_err(store_err("could not attach the tag"))?;
        }
        tx.commit().map_err(store_err("could not save the tags"))
    }

    /// The tags on a rule.
    pub fn rule_tags(&self, rule: RuleId) -> Result<Vec<Tag>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, t.name, t.colour FROM tag t
                 JOIN rule_tag rt ON rt.tag_id = t.id
                 WHERE rt.rule_id = ?1 ORDER BY t.name",
            )
            .map_err(store_err("could not read the rule's tags"))?;
        let rows = stmt
            .query_map([rule], |row| {
                Ok(Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    colour: row.get(2)?,
                })
            })
            .map_err(store_err("could not read the rule's tags"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a tag row"))
    }

    // -- rules ------------------------------------------------------------

    /// Creates a rule and returns its id.
    pub fn create_rule(&self, spec: &RuleSpec, created_at: i64) -> Result<RuleId> {
        let (retention_kind, retention_value) = spec.retention.to_columns();
        let excludes = serde_json::to_string(&spec.excludes)
            .map_err(|e| CoreError::Store(format!("could not encode the exclude list: {e}")))?;

        self.conn
            .execute(
                "INSERT INTO rule (name, enabled, source_volume, source_rel, layout, packaging,
                                   allow_deletions, retention_kind, retention_value, schedule,
                                   run_on_connect, catch_up, placeholders, hydrate_budget_bytes,
                                   follow_symlinks, excludes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                rusqlite::params![
                    spec.name,
                    i64::from(spec.enabled),
                    spec.source.volume,
                    path_to_db(&spec.source.relative),
                    spec.layout,
                    spec.packaging,
                    i64::from(spec.allow_deletions),
                    retention_kind,
                    retention_value,
                    spec.schedule.to_db_string(),
                    i64::from(spec.run_on_connect),
                    i64::from(spec.catch_up),
                    spec.placeholders,
                    spec.hydrate_budget_bytes.map(as_i64),
                    i64::from(spec.follow_symlinks),
                    excludes,
                    created_at,
                ],
            )
            .map_err(store_err("could not create the rule"))?;
        Ok(RuleId(self.conn.last_insert_rowid()))
    }

    /// Replaces a rule's configuration, keeping its id and creation time.
    pub fn update_rule(&self, id: RuleId, spec: &RuleSpec) -> Result<()> {
        let (retention_kind, retention_value) = spec.retention.to_columns();
        let excludes = serde_json::to_string(&spec.excludes)
            .map_err(|e| CoreError::Store(format!("could not encode the exclude list: {e}")))?;

        let n = self
            .conn
            .execute(
                "UPDATE rule SET name = ?2, enabled = ?3, source_volume = ?4, source_rel = ?5,
                                 layout = ?6, packaging = ?7, allow_deletions = ?8,
                                 retention_kind = ?9, retention_value = ?10, schedule = ?11,
                                 run_on_connect = ?12, catch_up = ?13, placeholders = ?14,
                                 hydrate_budget_bytes = ?15, follow_symlinks = ?16, excludes = ?17
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    spec.name,
                    i64::from(spec.enabled),
                    spec.source.volume,
                    path_to_db(&spec.source.relative),
                    spec.layout,
                    spec.packaging,
                    i64::from(spec.allow_deletions),
                    retention_kind,
                    retention_value,
                    spec.schedule.to_db_string(),
                    i64::from(spec.run_on_connect),
                    i64::from(spec.catch_up),
                    spec.placeholders,
                    spec.hydrate_budget_bytes.map(as_i64),
                    i64::from(spec.follow_symlinks),
                    excludes,
                ],
            )
            .map_err(store_err("could not update the rule"))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("rule {id}")));
        }
        Ok(())
    }

    /// One rule by id.
    pub fn rule(&self, id: RuleId) -> Result<Rule> {
        self.conn
            .query_row(&format!("{RULE_SELECT} WHERE id = ?1"), [id], read_rule)
            .optional()
            .map_err(store_err("could not read the rule"))?
            .ok_or_else(|| CoreError::NotFound(format!("rule {id}")))?
    }

    /// Every rule, oldest first.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{RULE_SELECT} ORDER BY id"))
            .map_err(store_err("could not list rules"))?;
        let rows = stmt
            .query_map([], read_rule)
            .map_err(store_err("could not list rules"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a rule row"))?
            .into_iter()
            .collect()
    }

    /// Deletes a rule, cascading to its destinations, runs and tag links.
    pub fn delete_rule(&self, id: RuleId) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM rule WHERE id = ?1", [id])
            .map_err(store_err("could not delete the rule"))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("rule {id}")));
        }
        Ok(())
    }

    // -- destinations -----------------------------------------------------

    /// Adds a destination to a rule.
    pub fn add_destination(
        &self,
        rule: RuleId,
        path: &VolumePath,
        sort_order: i64,
    ) -> Result<DestinationId> {
        self.conn
            .execute(
                "INSERT INTO destination (rule_id, volume_id, dest_rel, sort_order)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![rule, path.volume, path_to_db(&path.relative), sort_order],
            )
            .map_err(store_err("could not add the destination"))?;
        Ok(DestinationId(self.conn.last_insert_rowid()))
    }

    /// A rule's destinations, in display order.
    pub fn destinations(&self, rule: RuleId) -> Result<Vec<Destination>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, rule_id, volume_id, dest_rel, sort_order
                 FROM destination WHERE rule_id = ?1 ORDER BY sort_order, id",
            )
            .map_err(store_err("could not list destinations"))?;
        let rows = stmt
            .query_map([rule], |row| {
                Ok(Destination {
                    id: row.get(0)?,
                    rule: row.get(1)?,
                    path: VolumePath {
                        volume: row.get(2)?,
                        relative: path_from_db(&row.get::<_, String>(3)?),
                    },
                    sort_order: row.get(4)?,
                })
            })
            .map_err(store_err("could not list destinations"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a destination row"))
    }

    /// Removes a destination.
    pub fn delete_destination(&self, id: DestinationId) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM destination WHERE id = ?1", [id])
            .map_err(store_err("could not delete the destination"))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("destination {id}")));
        }
        Ok(())
    }

    // -- runs -------------------------------------------------------------

    /// Records the start of a run. `result` stays `NULL` until it finishes.
    pub fn begin_run(
        &self,
        rule: RuleId,
        destination: DestinationId,
        trigger: RunTrigger,
        started_at: i64,
    ) -> Result<RunId> {
        self.conn
            .execute(
                "INSERT INTO run (rule_id, destination_id, \"trigger\", started_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![rule, destination, trigger, started_at],
            )
            .map_err(store_err("could not record the start of the run"))?;
        Ok(RunId(self.conn.last_insert_rowid()))
    }

    /// Records the outcome of a run.
    pub fn finish_run(
        &self,
        id: RunId,
        result: RunResult,
        stats: &RunStats,
        finished_at: i64,
        error: Option<&str>,
        snapshot_path: Option<&Path>,
    ) -> Result<()> {
        let n = self
            .conn
            .execute(
                "UPDATE run SET result = ?2, finished_at = ?3, error = ?4, snapshot_path = ?5,
                                files_copied = ?6, files_skipped = ?7, files_deleted = ?8,
                                bytes_copied = ?9, compressed_bytes = ?10,
                                placeholders_hydrated = ?11, placeholders_skipped = ?12,
                                bytes_hydrated = ?13, bytes_released = ?14
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    result,
                    finished_at,
                    error,
                    snapshot_path.map(path_to_db),
                    as_i64(stats.files_copied),
                    as_i64(stats.files_skipped),
                    as_i64(stats.files_deleted),
                    as_i64(stats.bytes_copied),
                    stats.compressed_bytes.map(as_i64),
                    as_i64(stats.placeholders_hydrated),
                    as_i64(stats.placeholders_skipped),
                    as_i64(stats.bytes_hydrated),
                    as_i64(stats.bytes_released),
                ],
            )
            .map_err(store_err("could not record the outcome of the run"))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    /// A rule's runs, newest first.
    pub fn runs_for_rule(&self, rule: RuleId, limit: u32) -> Result<Vec<Run>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "{RUN_SELECT} WHERE rule_id = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2"
            ))
            .map_err(store_err("could not list runs"))?;
        let rows = stmt
            .query_map(rusqlite::params![rule, i64::from(limit)], read_run)
            .map_err(store_err("could not list runs"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a run row"))
    }

    /// A rule's most recent run, if it has ever run.
    ///
    /// This is what the table's Last Result column shows.
    pub fn last_run(&self, rule: RuleId) -> Result<Option<Run>> {
        self.conn
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE rule_id = ?1 ORDER BY started_at DESC, id DESC LIMIT 1"
                ),
                [rule],
                read_run,
            )
            .optional()
            .map_err(store_err("could not read the last run"))
    }

    /// Records a per-file problem.
    pub fn add_run_event(&self, event: &RunEvent) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO run_event (run_id, level, path, message) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    event.run,
                    event.level,
                    path_to_db(&event.path),
                    event.message
                ],
            )
            .map_err(store_err("could not record the run event"))?;
        Ok(())
    }

    /// The problems recorded against a run.
    pub fn run_events(&self, run: RunId) -> Result<Vec<RunEvent>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT run_id, level, path, message FROM run_event WHERE run_id = ?1 ORDER BY id",
            )
            .map_err(store_err("could not list run events"))?;
        let rows = stmt
            .query_map([run], |row| {
                Ok(RunEvent {
                    run: row.get(0)?,
                    level: row.get::<_, EventLevel>(1)?,
                    path: path_from_db(&row.get::<_, String>(2)?),
                    message: row.get(3)?,
                })
            })
            .map_err(store_err("could not list run events"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_err("could not read a run event row"))
    }
}

const RULE_SELECT: &str = "SELECT id, name, enabled, source_volume, source_rel, layout, packaging,
            allow_deletions, retention_kind, retention_value, schedule, run_on_connect,
            catch_up, placeholders, hydrate_budget_bytes, follow_symlinks, excludes, created_at
     FROM rule";

const RUN_SELECT: &str = "SELECT id, rule_id, destination_id, \"trigger\", started_at, finished_at,
            result, files_copied, files_skipped, files_deleted, bytes_copied, compressed_bytes,
            placeholders_hydrated, placeholders_skipped, bytes_hydrated, bytes_released,
            error, snapshot_path
     FROM run";

/// Encodes a path for storage.
///
/// Paths are stored as the platform writes them. A non-UTF-8 path would be
/// mangled by a lossy conversion, so it is rejected at the boundary instead.
fn path_to_db(path: &Path) -> String {
    path.to_str().map_or_else(
        || {
            #[allow(
                clippy::disallowed_methods,
                reason = "non-UTF-8 paths are rejected before storage; this is the \
                          diagnostic path only"
            )]
            path.to_string_lossy().into_owned()
        },
        ToOwned::to_owned,
    )
}

fn path_from_db(text: &str) -> PathBuf {
    PathBuf::from(text)
}

fn drive_type_str(t: DriveType) -> &'static str {
    match t {
        DriveType::Fixed => "fixed",
        DriveType::Removable => "removable",
        DriveType::Network => "network",
        DriveType::Optical => "optical",
        DriveType::RamDisk => "ramdisk",
        DriveType::Unknown => "unknown",
    }
}

fn drive_type_from_str(s: &str) -> DriveType {
    match s {
        "fixed" => DriveType::Fixed,
        "removable" => DriveType::Removable,
        "network" => DriveType::Network,
        "optical" => DriveType::Optical,
        "ramdisk" => DriveType::RamDisk,
        // An unrecognised value reads as Unknown, which is refused. Failing
        // towards refusal is the safe direction (`docs/PLAN.md` §4.2).
        _ => DriveType::Unknown,
    }
}

fn serde_plain_kind(kind: VolumeIdentityKind) -> &'static str {
    match kind {
        VolumeIdentityKind::WindowsVolumeGuid => "win_volume_guid",
        VolumeIdentityKind::LinuxFsUuid => "linux_fs_uuid",
    }
}

fn kind_from_str(s: &str) -> VolumeIdentityKind {
    match s {
        "linux_fs_uuid" => VolumeIdentityKind::LinuxFsUuid,
        _ => VolumeIdentityKind::WindowsVolumeGuid,
    }
}

fn read_volume(row: &Row<'_>) -> rusqlite::Result<StoredVolume> {
    Ok(StoredVolume {
        id: row.get(0)?,
        identity: VolumeIdentity {
            kind: kind_from_str(&row.get::<_, String>(1)?),
            value: row.get(2)?,
        },
        serial: row.get(3)?,
        label: row.get(4)?,
        filesystem: row.get(5)?,
        drive_type: drive_type_from_str(&row.get::<_, String>(6)?),
        is_sync_root: row.get::<_, i64>(7)? != 0,
        last_seen_at: row.get(8)?,
        last_mount: row.get::<_, Option<String>>(9)?.map(|s| path_from_db(&s)),
    })
}

/// Reads a rule row. The outer `rusqlite::Result` covers column access; the
/// inner [`Result`] covers values `SQLite` accepted but the domain cannot.
fn read_rule(row: &Row<'_>) -> rusqlite::Result<Result<Rule>> {
    let schedule_text: String = row.get(10)?;
    let excludes_text: String = row.get(16)?;
    let retention_kind: Option<String> = row.get(8)?;
    let retention_value: Option<u32> = row.get(9)?;

    let Some(schedule) = Schedule::from_db_str(&schedule_text) else {
        return Ok(Err(CoreError::Store(format!(
            "unknown schedule in database: {schedule_text}"
        ))));
    };
    let excludes = match serde_json::from_str::<Vec<String>>(&excludes_text) {
        Ok(v) => v,
        Err(e) => {
            return Ok(Err(CoreError::Store(format!(
                "could not decode the exclude list: {e}"
            ))))
        }
    };

    Ok(Ok(Rule {
        id: row.get(0)?,
        created_at: row.get(17)?,
        spec: RuleSpec {
            name: row.get(1)?,
            enabled: row.get::<_, i64>(2)? != 0,
            source: VolumePath {
                volume: row.get(3)?,
                relative: path_from_db(&row.get::<_, String>(4)?),
            },
            layout: row.get::<_, Layout>(5)?,
            packaging: row.get::<_, Packaging>(6)?,
            allow_deletions: row.get::<_, i64>(7)? != 0,
            retention: Retention::from_columns(retention_kind.as_deref(), retention_value),
            schedule,
            run_on_connect: row.get::<_, i64>(11)? != 0,
            catch_up: row.get::<_, i64>(12)? != 0,
            placeholders: row.get::<_, PlaceholderPolicy>(13)?,
            hydrate_budget_bytes: row.get::<_, Option<i64>>(14)?.map(as_u64),
            follow_symlinks: row.get::<_, i64>(15)? != 0,
            excludes,
        },
    }))
}

fn read_run(row: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: row.get(0)?,
        rule: row.get(1)?,
        destination: row.get(2)?,
        trigger: row.get::<_, RunTrigger>(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        result: row.get::<_, Option<RunResult>>(6)?,
        stats: RunStats {
            files_copied: as_u64(row.get(7)?),
            files_skipped: as_u64(row.get(8)?),
            files_deleted: as_u64(row.get(9)?),
            bytes_copied: as_u64(row.get(10)?),
            compressed_bytes: row.get::<_, Option<i64>>(11)?.map(as_u64),
            placeholders_hydrated: as_u64(row.get(12)?),
            placeholders_skipped: as_u64(row.get(13)?),
            bytes_hydrated: as_u64(row.get(14)?),
            bytes_released: as_u64(row.get(15)?),
        },
        error: row.get(16)?,
        snapshot_path: row.get::<_, Option<String>>(17)?.map(|s| path_from_db(&s)),
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests;

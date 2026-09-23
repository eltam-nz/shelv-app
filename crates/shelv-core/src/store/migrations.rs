//! Schema migrations, keyed on `SQLite`'s `user_version`.
//!
//! Rules:
//!
//! * migrations are append-only — never edit one that has shipped;
//! * each runs in a transaction, so a failure leaves the version unchanged;
//! * `user_version` is set inside that transaction, so it cannot drift from
//!   the schema it describes.

use rusqlite::{Connection, Transaction};

use crate::error::CoreError;
use crate::Result;

/// A numbered schema step. `version` is what `user_version` becomes once
/// `sql` has been applied.
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// Every migration, in order.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial schema",
        sql: include_str!("../../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "layout implies deletions",
        sql: include_str!("../../migrations/0002_layout_implies_deletions.sql"),
    },
    Migration {
        version: 3,
        name: "volume nickname",
        sql: include_str!("../../migrations/0003_volume_nickname.sql"),
    },
    Migration {
        version: 4,
        name: "period schedules",
        sql: include_str!("../../migrations/0004_period_schedules.sql"),
    },
    Migration {
        version: 5,
        name: "refused runs",
        sql: include_str!("../../migrations/0005_refused_runs.sql"),
    },
];

/// The schema version this build expects.
pub const LATEST_VERSION: i64 = 5;

/// Applies any migrations the database has not yet seen.
///
/// Returns the version it ended on.
pub fn migrate(conn: &mut Connection) -> Result<i64> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| CoreError::Store(format!("could not read the schema version: {e}")))?;

    if current > LATEST_VERSION {
        // Opening a newer database with an older build would silently write
        // rows the newer schema cannot read. Refuse instead.
        return Err(CoreError::Store(format!(
            "this database is at schema version {current}, but this build of Shelv only \
             understands up to {LATEST_VERSION}. Upgrade Shelv to open it."
        )));
    }

    for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = conn
            .transaction()
            .map_err(|e| CoreError::Store(format!("could not begin a migration: {e}")))?;
        apply(&tx, migration)?;
        tx.commit().map_err(|e| {
            CoreError::Store(format!(
                "could not commit migration {} ({}): {e}",
                migration.version, migration.name
            ))
        })?;
        tracing::info!(
            version = migration.version,
            name = migration.name,
            "applied migration"
        );
    }

    Ok(LATEST_VERSION)
}

fn apply(tx: &Transaction<'_>, migration: &Migration) -> Result<()> {
    tx.execute_batch(migration.sql).map_err(|e| {
        CoreError::Store(format!(
            "migration {} ({}) failed: {e}",
            migration.version, migration.name
        ))
    })?;
    // Bound as a literal because PRAGMA does not accept a parameter.
    tx.execute_batch(&format!("PRAGMA user_version = {};", migration.version))
        .map_err(|e| {
            CoreError::Store(format!(
                "could not record schema version {}: {e}",
                migration.version
            ))
        })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_contiguous_and_ordered() {
        // A gap or a repeat means some databases silently skip a step.
        for (i, m) in MIGRATIONS.iter().enumerate() {
            let expected = i64::try_from(i).unwrap() + 1;
            assert_eq!(m.version, expected, "migration {} is out of order", m.name);
        }
        assert_eq!(
            MIGRATIONS.last().map(|m| m.version),
            Some(LATEST_VERSION),
            "LATEST_VERSION must match the last migration"
        );
    }

    #[test]
    fn migrating_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), LATEST_VERSION);
        // Running again must be a no-op rather than re-applying DDL.
        assert_eq!(migrate(&mut conn).unwrap(), LATEST_VERSION);
    }

    #[test]
    fn an_existing_v1_database_upgrades_in_place() {
        // The upgrade path real users take, and the one no fresh-install
        // test exercises: v1 shipped, so a database already carrying
        // `allow_deletions` has to survive the column being dropped with its
        // rules intact. It also pins that the bundled SQLite is new enough
        // for ALTER TABLE ... DROP COLUMN (3.35+).
        let mut conn = Connection::open_in_memory().unwrap();
        apply_up_to(&mut conn, 1);
        conn.execute_batch(
            "INSERT INTO volume (identity_kind, identity, drive_type)
             VALUES ('linux_fs_uuid', 'uuid-1', 'removable');
             INSERT INTO rule (name, source_volume, source_rel, layout, packaging,
                               allow_deletions, schedule, created_at)
             VALUES ('Photos', 1, 'Pictures', 'mirror', 'files', 1, 'weekly', 1000);",
        )
        .unwrap();

        assert_eq!(migrate(&mut conn).unwrap(), LATEST_VERSION);

        let name: String = conn
            .query_row("SELECT name FROM rule", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "Photos", "the rule must survive the migration");
        assert!(
            conn.query_row("SELECT allow_deletions FROM rule", [], |row| row
                .get::<_, i64>(0))
                .is_err(),
            "the column should be gone"
        );
    }

    #[test]
    fn a_v3_database_loses_catch_up_and_keeps_its_rules() {
        // The column has a DEFAULT, so a build that stopped writing it would
        // keep inserting rows happily while the column quietly persisted —
        // which is exactly what happened before this test existed, and why
        // registering the migration cannot be left to a fresh-install test.
        let mut conn = Connection::open_in_memory().unwrap();
        apply_up_to(&mut conn, 3);
        conn.execute_batch(
            "INSERT INTO volume (identity_kind, identity, drive_type)
             VALUES ('linux_fs_uuid', 'uuid-1', 'removable');
             INSERT INTO rule (name, source_volume, source_rel, layout, packaging,
                               catch_up, schedule, created_at)
             VALUES ('Photos', 1, 'Pictures', 'mirror', 'files', 1, 'daily', 1000);",
        )
        .unwrap();

        assert_eq!(migrate(&mut conn).unwrap(), LATEST_VERSION);

        let (name, schedule): (String, String) = conn
            .query_row("SELECT name, schedule FROM rule", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "Photos");
        assert_eq!(schedule, "daily", "the schedule itself is unchanged");
        assert!(
            conn.query_row("SELECT catch_up FROM rule", [], |row| row.get::<_, i64>(0))
                .is_err(),
            "the column should be gone"
        );
    }

    #[test]
    fn a_v4_database_keeps_its_run_history_through_the_table_rebuild() {
        // SQLite cannot alter a CHECK, so 0005 copies the table. That is the
        // one migration so far that could lose data rather than a column,
        // and run history is the record someone consults after a drive
        // fails — so it is checked rather than assumed, indexes included.
        let mut conn = Connection::open_in_memory().unwrap();
        apply_up_to(&mut conn, 4);
        conn.execute_batch(
            "INSERT INTO volume (identity_kind, identity, drive_type)
             VALUES ('linux_fs_uuid', 'uuid-1', 'removable');
             INSERT INTO rule (name, source_volume, source_rel, layout, packaging,
                               schedule, created_at)
             VALUES ('Photos', 1, 'Pictures', 'mirror', 'files', 'daily', 1000);
             INSERT INTO destination (rule_id, volume_id, dest_rel, sort_order)
             VALUES (1, 1, 'Backups', 0);
             INSERT INTO run (rule_id, destination_id, \"trigger\", started_at,
                              finished_at, result, files_copied, bytes_copied)
             VALUES (1, 1, 'manual', 2000, 2100, 'partial', 7, 4096);",
        )
        .unwrap();

        assert_eq!(migrate(&mut conn).unwrap(), LATEST_VERSION);

        let (result, copied, bytes): (String, i64, i64) = conn
            .query_row(
                "SELECT result, files_copied, bytes_copied FROM run",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((result.as_str(), copied, bytes), ("partial", 7, 4096));

        // The new spelling is now accepted, which is the point of the rebuild.
        conn.execute(
            "INSERT INTO run (rule_id, destination_id, \"trigger\", started_at, result)
             VALUES (1, 1, 'schedule', 3000, 'refused')",
            [],
        )
        .unwrap();

        // And the indexes came back with it.
        let indexes: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'run' AND name LIKE 'idx_run%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 2, "both indexes must survive the rebuild");
    }

    /// Brings a connection to a given schema version, as a shipped build of
    /// that version would have left it.
    fn apply_up_to(conn: &mut Connection, version: i64) {
        for migration in MIGRATIONS.iter().filter(|m| m.version <= version) {
            let tx = conn.transaction().unwrap();
            apply(&tx, migration).unwrap();
            tx.commit().unwrap();
        }
    }

    #[test]
    fn a_v2_database_gains_the_nickname_column_without_losing_its_drives() {
        // Somebody who installed the previous build already has drives
        // recorded. The nickname column arriving must not disturb them, and
        // must arrive empty rather than guessing a name.
        let mut conn = Connection::open_in_memory().unwrap();
        apply_up_to(&mut conn, 2);
        conn.execute_batch(
            "INSERT INTO volume (identity_kind, identity, label, drive_type)
             VALUES ('linux_fs_uuid', 'uuid-1', 'Expansion', 'removable');",
        )
        .unwrap();

        assert_eq!(migrate(&mut conn).unwrap(), LATEST_VERSION);

        let (label, nickname): (String, Option<String>) = conn
            .query_row("SELECT label, nickname FROM volume", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(label, "Expansion", "the drive must survive the migration");
        assert_eq!(nickname, None, "no nickname is not the same as a guess");
    }

    #[test]
    fn a_future_database_is_refused_not_downgraded() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 999;").unwrap();
        let err = migrate(&mut conn).unwrap_err();
        assert!(
            format!("{err}").contains("Upgrade Shelv"),
            "expected an upgrade prompt, got: {err}"
        );
    }
}

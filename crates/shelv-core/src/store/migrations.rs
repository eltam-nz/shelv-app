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
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial schema",
    sql: include_str!("../../migrations/0001_initial.sql"),
}];

/// The schema version this build expects.
pub const LATEST_VERSION: i64 = 1;

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

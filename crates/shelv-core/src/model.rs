//! The domain model: rules, destinations, tags, volumes and run history.
//!
//! These types mirror the schema in `docs/PLAN.md` §2.3. Identifiers are
//! newtypes rather than bare `i64` so that a rule id cannot be passed where a
//! destination id is expected — the IPC surface deals almost entirely in ids
//! (`docs/PLAN.md` §4, T1), and there the distinction is load-bearing.

use std::path::PathBuf;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::ToSql;
use serde::{Deserialize, Serialize};

use crate::platform::VolumeIdentity;

/// Declares a database-backed enum with its stored spelling.
///
/// The stored strings are part of the on-disk format: changing one is a
/// migration, not a rename. Round-tripping is checked by a generated test.
macro_rules! sql_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
        #[ts(export, export_to = "../../../src/types/")]
        #[serde(rename_all = "snake_case")]
        $vis enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// How this value is spelled in the database.
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self { $( Self::$variant => $text ),+ }
            }

            /// Parses the database spelling.
            #[must_use]
            pub fn from_db_str(s: &str) -> Option<Self> {
                match s { $( $text => Some(Self::$variant), )+ _ => None }
            }

            /// Every variant, for exhaustive tests.
            #[must_use]
            pub const fn all() -> &'static [Self] {
                &[ $( Self::$variant ),+ ]
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.as_db_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let text = value.as_str()?;
                Self::from_db_str(text).ok_or_else(|| {
                    FromSqlError::Other(Box::new(crate::CoreError::Store(format!(
                        concat!("unknown ", stringify!($name), " in database: {}"),
                        text
                    ))))
                })
            }
        }
    };
}

/// Declares a strongly typed row identifier.
macro_rules! row_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ts_rs::TS)]
        #[ts(export, export_to = "../../../src/types/")]
        // serde_json writes an i64 as a JSON number, so `JSON.parse` hands the
        // frontend a `number`. ts-rs would otherwise emit `bigint`, a type the
        // wire format never actually produces.
        #[ts(type = "number")]
        pub struct $name(pub i64);

        // Serialised as a bare integer, the same as `#[serde(transparent)]`
        // would give. Written out by hand because ts-rs cannot parse that
        // attribute and warns on every use of it.
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                i64::deserialize(d).map(Self)
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                value.as_i64().map(Self)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

row_id!(
    /// Identifies a row in `volume`.
    VolumeId
);
row_id!(
    /// Identifies a row in `rule`.
    RuleId
);
row_id!(
    /// Identifies a row in `destination`.
    DestinationId
);
row_id!(
    /// Identifies a row in `tag`.
    TagId
);
row_id!(
    /// Identifies a row in `run`.
    RunId
);

sql_enum! {
    /// How a backup is laid out at the destination.
    pub enum Layout {
        /// The destination mirrors the source in place.
        Mirror => "mirror",
        /// Each run writes a new timestamped subfolder.
        Snapshot => "snapshot",
    }
}

impl Layout {
    /// Whether a run may remove files the source no longer has.
    ///
    /// This follows from the layout rather than being asked separately. A
    /// mirror that never deletes is not a mirror: it drifts from the source
    /// with every run, so the one thing the user asked for stops happening,
    /// silently. A snapshot is the opposite case — each folder records a
    /// moment, and editing a past moment is not a backup.
    ///
    /// Deletion still means the recycle bin, via
    /// [`PlatformFs::trash`](crate::platform::PlatformFs::trash), never an
    /// unlink, so a mis-aimed mirror stays recoverable (`docs/PLAN.md` §4,
    /// T4). Pruning old *snapshots* is a different operation, governed by
    /// [`Retention`], and only ever removes whole snapshot folders.
    #[must_use]
    pub const fn deletes_extraneous(self) -> bool {
        match self {
            Self::Mirror => true,
            Self::Snapshot => false,
        }
    }
}

sql_enum! {
    /// Whether files are copied as-is or packed into an archive.
    ///
    /// Every option is lossless. `ZipStore` packs without compressing, which is
    /// the right choice for RAW and JPEG: they are already entropy-coded, so
    /// deflate costs full CPU for a few percent (`docs/PLAN.md` §1.1b).
    pub enum Packaging {
        /// Plain files and folders.
        Files => "files",
        /// A Zip64 archive, stored without compression.
        ZipStore => "zip_store",
        /// A Zip64 archive, deflated.
        ZipDeflate => "zip_deflate",
    }
}

sql_enum! {
    /// What to do about cloud files whose content is not on local disk.
    pub enum PlaceholderPolicy {
        /// Download, back up, and leave the local copy in place.
        Hydrate => "hydrate",
        /// Download, back up, then ask the sync engine to reclaim the space.
        HydrateRelease => "hydrate_release",
        /// Leave placeholders alone and report them as skipped.
        Skip => "skip",
    }
}

sql_enum! {
    /// What caused a run to start.
    pub enum RunTrigger {
        /// The user pressed Backup Now.
        Manual => "manual",
        /// The rule's schedule came due.
        Schedule => "schedule",
        /// A scheduled run had been missed and was made up.
        CatchUp => "catch_up",
        /// A volume the rule depends on was attached.
        OnConnect => "on_connect",
    }
}

sql_enum! {
    /// How a run finished.
    pub enum RunResult {
        /// Every file was copied.
        Ok => "ok",
        /// The run completed but some files could not be read or written. A
        /// backup tool that cannot tell this from success is worse than none.
        Partial => "partial",
        /// The run did not complete.
        Failed => "failed",
        /// The user cancelled, or the volume went away.
        Cancelled => "cancelled",
    }
}

sql_enum! {
    /// Severity of a per-file problem recorded during a run.
    pub enum EventLevel {
        /// The file was skipped; the run continues and ends `Partial`.
        Warn => "warn",
        /// The failure stopped the run.
        Error => "error",
    }
}

/// How often a rule wants to run.
///
/// This is an upper bound on frequency, not a promise: a rule whose destination
/// is unplugged runs when the drive next appears (`docs/PLAN.md` §1.1d).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Schedule {
    /// Only when the user asks.
    Manual,
    /// Once a day.
    Daily,
    /// Once a week.
    Weekly,
    /// Once a month.
    Monthly,
    /// A cron expression, for anything else.
    Cron(String),
}

impl Schedule {
    /// The database spelling. Cron expressions are stored as `cron:<expr>`.
    #[must_use]
    pub fn to_db_string(&self) -> String {
        match self {
            Self::Manual => "manual".to_owned(),
            Self::Daily => "daily".to_owned(),
            Self::Weekly => "weekly".to_owned(),
            Self::Monthly => "monthly".to_owned(),
            Self::Cron(expr) => format!("cron:{expr}"),
        }
    }

    /// Parses the database spelling.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(Self::Manual),
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            other => other
                .strip_prefix("cron:")
                .map(|e| Self::Cron(e.to_owned())),
        }
    }
}

/// How many snapshots to keep.
///
/// Snapshots accumulate until the drive is full, so retention is part of that
/// layout rather than an optional extra (`docs/PLAN.md` §1.1c). Pruning happens
/// only after a run succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Retention {
    /// Keep everything. Only sensible for `Layout::Mirror`.
    Unlimited,
    /// Keep the newest `n` snapshots.
    KeepLastN(u32),
    /// Keep snapshots newer than `n` days.
    KeepDays(u32),
}

impl Retention {
    /// Splits into the `(retention_kind, retention_value)` column pair.
    #[must_use]
    pub const fn to_columns(self) -> (Option<&'static str>, Option<u32>) {
        match self {
            Self::Unlimited => (None, None),
            Self::KeepLastN(n) => (Some("keep_last_n"), Some(n)),
            Self::KeepDays(n) => (Some("keep_days"), Some(n)),
        }
    }

    /// Rebuilds from the column pair. An unrecognised or incomplete pair reads
    /// as [`Unlimited`](Self::Unlimited), which never deletes anything.
    #[must_use]
    pub fn from_columns(kind: Option<&str>, value: Option<u32>) -> Self {
        match (kind, value) {
            (Some("keep_last_n"), Some(n)) => Self::KeepLastN(n),
            (Some("keep_days"), Some(n)) => Self::KeepDays(n),
            _ => Self::Unlimited,
        }
    }
}

/// A location expressed as a volume plus a path relative to its mount point.
///
/// Never an absolute path: the mount point moves between sessions, the volume
/// identity does not (`docs/PLAN.md` §1.1a).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct VolumePath {
    /// The volume this path lives on.
    pub volume: VolumeId,
    /// Path relative to the volume's mount point.
    pub relative: PathBuf,
}

/// A volume as recorded in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct StoredVolume {
    /// Row id.
    pub id: VolumeId,
    /// Stable identity, as produced by the platform layer.
    pub identity: VolumeIdentity,
    /// Volume serial, where the platform exposes one.
    pub serial: Option<String>,
    /// Human-readable label, as the operating system reports it.
    pub label: Option<String>,
    /// A name the user gave this drive, or `None` if they have not.
    ///
    /// **Display only.** Nothing matches, resolves or writes on it: the
    /// identity remains the only key anything is looked up by. A name that
    /// decided where a backup went would reintroduce exactly the wrong-drive
    /// failure identity matching exists to prevent (`docs/PLAN.md` §4, T3).
    pub nickname: Option<String>,
    /// Filesystem name.
    pub filesystem: Option<String>,
    /// What backs the volume, as last seen.
    pub drive_type: crate::platform::DriveType,
    /// Whether this volume is, or contains, a cloud sync root.
    pub is_sync_root: bool,
    /// When the volume was last seen attached, as a Unix timestamp.
    #[ts(type = "number | null")]
    pub last_seen_at: Option<i64>,
    /// Where it was last mounted. Display only.
    pub last_mount: Option<PathBuf>,
}

/// A colour-coded tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Tag {
    /// Row id.
    pub id: TagId,
    /// Display name.
    pub name: String,
    /// Palette token, not a raw colour: the UI resolves it against the
    /// contrast-checked theme so a tag cannot be made unreadable.
    pub colour: String,
}

/// A backup destination belonging to a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Destination {
    /// Row id.
    pub id: DestinationId,
    /// The rule this destination belongs to.
    pub rule: RuleId,
    /// Where the backup is written.
    pub path: VolumePath,
    /// Display order within the rule.
    #[ts(type = "number")]
    pub sort_order: i64,
}

/// Everything about a rule except its identity.
///
/// Separated from [`Rule`] so that creating and updating take the same type,
/// with no placeholder id and no duplicated field list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these are independent user-facing settings, not a state machine"
)]
pub struct RuleSpec {
    /// Display name.
    pub name: String,
    /// Whether the scheduler considers this rule at all.
    pub enabled: bool,
    /// What is being backed up.
    pub source: VolumePath,
    /// How the backup is laid out.
    pub layout: Layout,
    /// Whether files are archived.
    pub packaging: Packaging,
    /// How many snapshots to keep.
    pub retention: Retention,
    /// How often to run.
    pub schedule: Schedule,
    /// Run when a destination volume is attached.
    pub run_on_connect: bool,
    /// Make up runs missed while the machine was off or the drive absent.
    pub catch_up: bool,
    /// What to do about cloud placeholders.
    pub placeholders: PlaceholderPolicy,
    /// Cap on bytes hydrated in one run. `None` means no cap.
    #[ts(type = "number | null")]
    pub hydrate_budget_bytes: Option<u64>,
    /// Whether to follow symlinks and junctions.
    ///
    /// Off by default: following them lets a link inside the source tree
    /// redirect the copy outside it (`docs/PLAN.md` §4, T2).
    pub follow_symlinks: bool,
    /// Glob patterns excluded from the backup.
    pub excludes: Vec<String>,
}

/// A backup rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Rule {
    /// Row id.
    pub id: RuleId,
    /// The rule's configuration.
    pub spec: RuleSpec,
    /// When the rule was created, as a Unix timestamp.
    #[ts(type = "number")]
    pub created_at: i64,
}

/// Counters describing what a run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RunStats {
    /// Files written to the destination.
    #[ts(type = "number")]
    pub files_copied: u64,
    /// Files that could not be read or written, and were left out.
    #[ts(type = "number")]
    pub files_skipped: u64,
    /// Files removed from the destination.
    #[ts(type = "number")]
    pub files_deleted: u64,
    /// Bytes written.
    #[ts(type = "number")]
    pub bytes_copied: u64,
    /// Bytes the archive occupies, where packaging applied. Reported so the
    /// compression setting can be judged on evidence (`docs/PLAN.md` §1.1b).
    #[ts(type = "number | null")]
    pub compressed_bytes: Option<u64>,
    /// Cloud placeholders downloaded.
    #[ts(type = "number")]
    pub placeholders_hydrated: u64,
    /// Cloud placeholders left alone.
    #[ts(type = "number")]
    pub placeholders_skipped: u64,
    /// Bytes downloaded from the cloud.
    #[ts(type = "number")]
    pub bytes_hydrated: u64,
    /// Bytes released back to the cloud after copying.
    #[ts(type = "number")]
    pub bytes_released: u64,
}

/// One execution of a rule against one destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Run {
    /// Row id.
    pub id: RunId,
    /// The rule that ran.
    pub rule: RuleId,
    /// The destination written to.
    pub destination: DestinationId,
    /// What started it.
    pub trigger: RunTrigger,
    /// Start time, as a Unix timestamp.
    #[ts(type = "number")]
    pub started_at: i64,
    /// Finish time, or `None` while the run is in progress.
    #[ts(type = "number | null")]
    pub finished_at: Option<i64>,
    /// Outcome, or `None` while in progress.
    pub result: Option<RunResult>,
    /// What the run did.
    pub stats: RunStats,
    /// Failure message, where the run failed.
    pub error: Option<String>,
    /// The snapshot folder written, for `Layout::Snapshot`.
    pub snapshot_path: Option<PathBuf>,
}

/// A per-file problem recorded during a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RunEvent {
    /// The run this belongs to.
    pub run: RunId,
    /// How serious it was.
    pub level: EventLevel,
    /// The file concerned.
    pub path: PathBuf,
    /// What happened.
    pub message: String,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;

    /// The stored spellings are an on-disk format. A rename that is not also a
    /// migration silently orphans every existing row, so pin them here.
    #[test]
    fn enum_spellings_round_trip() {
        macro_rules! check {
            ($t:ty) => {
                for v in <$t>::all() {
                    assert_eq!(
                        <$t>::from_db_str(v.as_db_str()),
                        Some(*v),
                        "{} did not round trip",
                        v.as_db_str()
                    );
                }
            };
        }
        check!(Layout);
        check!(Packaging);
        check!(PlaceholderPolicy);
        check!(RunTrigger);
        check!(RunResult);
        check!(EventLevel);
    }

    #[test]
    fn only_a_mirror_removes_files_the_source_no_longer_has() {
        // The whole point of dropping `allow_deletions`: this is now decided
        // by the layout, and a snapshot can never reach a previous snapshot.
        assert!(Layout::Mirror.deletes_extraneous());
        assert!(!Layout::Snapshot.deletes_extraneous());
    }

    #[test]
    fn unknown_spellings_are_rejected() {
        assert_eq!(Layout::from_db_str("incremental"), None);
        assert_eq!(Packaging::from_db_str("rar"), None);
    }

    #[test]
    fn schedules_round_trip() {
        let cases = [
            Schedule::Manual,
            Schedule::Daily,
            Schedule::Weekly,
            Schedule::Monthly,
            Schedule::Cron("0 3 * * 1".to_owned()),
        ];
        for c in cases {
            assert_eq!(Schedule::from_db_str(&c.to_db_string()), Some(c.clone()));
        }
        assert_eq!(Schedule::from_db_str("fortnightly"), None);
    }

    #[test]
    fn retention_round_trips_through_its_column_pair() {
        for r in [
            Retention::Unlimited,
            Retention::KeepLastN(7),
            Retention::KeepDays(90),
        ] {
            let (kind, value) = r.to_columns();
            assert_eq!(Retention::from_columns(kind, value), r);
        }
    }

    #[test]
    fn a_broken_retention_pair_deletes_nothing() {
        // Half-written or unrecognised rows must fail towards keeping data.
        assert_eq!(
            Retention::from_columns(Some("keep_last_n"), None),
            Retention::Unlimited
        );
        assert_eq!(
            Retention::from_columns(Some("keep_every_other"), Some(3)),
            Retention::Unlimited
        );
    }
}

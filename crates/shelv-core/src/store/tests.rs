//! Store tests.
//!
//! These lean on the cases that would quietly lose data: cascades that do not
//! fire, enum values that do not survive a round trip, and the constraints
//! that stop a half-written rule from reading as a valid one.

use std::path::PathBuf;

use super::*;
use crate::model::{EventLevel, Layout, Packaging, PlaceholderPolicy, Retention, Schedule};
use crate::platform::VolumeIdentityKind;

fn volume_info(value: &str, mount: &str) -> VolumeInfo {
    VolumeInfo {
        identity: VolumeIdentity {
            kind: VolumeIdentityKind::WindowsVolumeGuid,
            value: value.to_owned(),
        },
        mount_point: PathBuf::from(mount),
        serial: Some("A1B2C3D4".to_owned()),
        label: Some("Backup Drive".to_owned()),
        filesystem: Some("exFAT".to_owned()),
        drive_type: DriveType::Removable,
        case_sensitivity: crate::platform::CaseSensitivity::Insensitive,
        is_sync_root: false,
    }
}

fn volume_id(store: &Store, value: &str) -> VolumeId {
    store
        .upsert_volume(&volume_info(value, "E:\\"), Some(1_700_000_000))
        .unwrap()
}

fn sample_spec(source: VolumeId) -> RuleSpec {
    RuleSpec {
        name: "Lightroom Catalog".to_owned(),
        enabled: true,
        source: VolumePath {
            volume: source,
            relative: PathBuf::from("Pictures/Lightroom"),
        },
        layout: Layout::Snapshot,
        packaging: Packaging::ZipDeflate,
        retention: Retention::KeepLastN(6),
        schedule: Schedule::Monthly,
        run_on_connect: true,
        placeholders: PlaceholderPolicy::HydrateRelease,
        hydrate_budget_bytes: Some(50 * 1024 * 1024 * 1024),
        follow_symlinks: false,
        excludes: vec!["*.lrprev".to_owned(), "Thumbs.db".to_owned()],
    }
}

#[test]
fn a_fresh_database_is_at_the_latest_version() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
}

#[test]
fn foreign_keys_are_enforced() {
    // SQLite disables these per connection by default. If the PRAGMA is ever
    // dropped, every ON DELETE CASCADE in the schema silently stops working.
    let store = Store::open_in_memory().unwrap();
    let enabled: i64 = store
        .conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(enabled, 1, "foreign key enforcement must be on");
}

#[test]
fn a_rule_round_trips_intact() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    let spec = sample_spec(vol);

    let id = store.create_rule(&spec, 1_700_000_000).unwrap();
    let read = store.rule(id).unwrap();

    assert_eq!(read.id, id);
    assert_eq!(read.created_at, 1_700_000_000);
    // Every field, so that adding one to RuleSpec without persisting it fails
    // here rather than in the UI.
    assert_eq!(read.spec, spec);
}

#[test]
fn updating_a_rule_keeps_its_id_and_creation_time() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    let id = store.create_rule(&sample_spec(vol), 111).unwrap();

    let mut changed = sample_spec(vol);
    changed.name = "Lightroom Catalog (weekly)".to_owned();
    changed.schedule = Schedule::Cron("0 3 * * 1".to_owned());
    changed.retention = Retention::KeepDays(30);
    changed.layout = Layout::Snapshot;
    changed.excludes.clear();
    store.update_rule(id, &changed).unwrap();

    let read = store.rule(id).unwrap();
    assert_eq!(read.id, id);
    assert_eq!(read.created_at, 111);
    assert_eq!(read.spec, changed);
}

#[test]
fn deleting_a_rule_cascades_to_everything_hanging_off_it() {
    let store = Store::open_in_memory().unwrap();
    let src = volume_id(&store, r"\\?\Volume{src}\");
    let target = volume_id(&store, r"\\?\Volume{dst}\");

    let rule = store.create_rule(&sample_spec(src), 1).unwrap();
    let destination = store
        .add_destination(
            rule,
            &VolumePath {
                volume: target,
                relative: PathBuf::from("Backups/Lightroom"),
            },
            0,
        )
        .unwrap();
    let tag = store.create_tag("images", "pastel-blue").unwrap();
    store.set_rule_tags(rule, &[tag]).unwrap();
    let run = store
        .begin_run(rule, destination, RunTrigger::Manual, 10)
        .unwrap();
    store
        .add_run_event(&RunEvent {
            run,
            level: EventLevel::Warn,
            path: PathBuf::from("a.lrcat"),
            message: "locked by another process".to_owned(),
        })
        .unwrap();

    store.delete_rule(rule).unwrap();

    let count = |sql: &str| -> i64 { store.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT COUNT(*) FROM destination"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM run"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM run_event"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM rule_tag"), 0);
    // The tag itself and the volumes outlive the rule.
    assert_eq!(count("SELECT COUNT(*) FROM tag"), 1);
    assert_eq!(count("SELECT COUNT(*) FROM volume"), 2);
}

#[test]
fn deleting_a_tag_unlinks_it_without_touching_the_rule() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    let rule = store.create_rule(&sample_spec(vol), 1).unwrap();
    let images = store.create_tag("images", "pastel-blue").unwrap();
    let important = store.create_tag("important", "pastel-rose").unwrap();
    store.set_rule_tags(rule, &[images, important]).unwrap();

    store.delete_tag(images).unwrap();

    let remaining = store.rule_tags(rule).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining.first().map(|t| t.name.as_str()),
        Some("important")
    );
    assert!(store.rule(rule).is_ok());
}

#[test]
fn a_volume_is_matched_on_identity_not_mount_point() {
    let store = Store::open_in_memory().unwrap();
    let first = store
        .upsert_volume(&volume_info(r"\\?\Volume{abcd}\", "E:\\"), Some(100))
        .unwrap();

    // Same drive, reconnected as F:. It must be recognised as the same row,
    // or rules pointing at it would silently target a new volume.
    let second = store
        .upsert_volume(&volume_info(r"\\?\Volume{abcd}\", "F:\\"), Some(200))
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(store.volumes().unwrap().len(), 1);
    let v = store.volume(first).unwrap();
    assert_eq!(v.last_mount, Some(PathBuf::from("F:\\")));
    assert_eq!(v.last_seen_at, Some(200));
}

#[test]
fn set_rule_tags_replaces_rather_than_appends() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    let rule = store.create_rule(&sample_spec(vol), 1).unwrap();
    let a = store.create_tag("a", "pastel-blue").unwrap();
    let b = store.create_tag("b", "pastel-mint").unwrap();

    store.set_rule_tags(rule, &[a, b]).unwrap();
    store.set_rule_tags(rule, &[b]).unwrap();

    let tags = store.rule_tags(rule).unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags.first().map(|t| t.id), Some(b));
}

#[test]
fn a_run_records_its_outcome_and_counters() {
    let store = Store::open_in_memory().unwrap();
    let src = volume_id(&store, r"\\?\Volume{src}\");
    let target = volume_id(&store, r"\\?\Volume{dst}\");
    let rule = store.create_rule(&sample_spec(src), 1).unwrap();
    let destination = store
        .add_destination(
            rule,
            &VolumePath {
                volume: target,
                relative: PathBuf::from("Backups"),
            },
            0,
        )
        .unwrap();

    let run = store
        .begin_run(rule, destination, RunTrigger::OnConnect, 1000)
        .unwrap();

    // In progress: no result yet.
    let open = store.last_run(rule).unwrap().unwrap();
    assert_eq!(open.result, None);
    assert_eq!(open.finished_at, None);
    assert_eq!(open.trigger, RunTrigger::OnConnect);

    let stats = RunStats {
        files_copied: 1200,
        files_skipped: 3,
        files_deleted: 0,
        bytes_copied: 9_000_000_000,
        compressed_bytes: Some(8_950_000_000),
        placeholders_hydrated: 40,
        placeholders_skipped: 1,
        bytes_hydrated: 2_000_000_000,
        bytes_released: 2_000_000_000,
        snapshots_pruned: 2,
    };
    store
        .finish_run(
            run,
            RunResult::Partial,
            &stats,
            1500,
            None,
            Some(Path::new("Backups/2026-09-20T0300")),
        )
        .unwrap();

    let done = store.last_run(rule).unwrap().unwrap();
    assert_eq!(done.result, Some(RunResult::Partial));
    assert_eq!(done.finished_at, Some(1500));
    assert_eq!(done.stats, stats);
    assert_eq!(
        done.snapshot_path,
        Some(PathBuf::from("Backups/2026-09-20T0300"))
    );
}

#[test]
fn last_run_is_the_most_recent_and_is_none_before_any() {
    let store = Store::open_in_memory().unwrap();
    let src = volume_id(&store, r"\\?\Volume{src}\");
    let rule = store.create_rule(&sample_spec(src), 1).unwrap();
    let destination = store
        .add_destination(
            rule,
            &VolumePath {
                volume: src,
                relative: PathBuf::from("Backups"),
            },
            0,
        )
        .unwrap();

    // "Never run" must be distinguishable from "ran and failed".
    assert!(store.last_run(rule).unwrap().is_none());

    store
        .begin_run(rule, destination, RunTrigger::Manual, 100)
        .unwrap();
    let newer = store
        .begin_run(rule, destination, RunTrigger::Schedule, 300)
        .unwrap();
    store
        .begin_run(rule, destination, RunTrigger::Manual, 200)
        .unwrap();

    assert_eq!(store.last_run(rule).unwrap().map(|r| r.id), Some(newer));
    assert_eq!(store.runs_for_rule(rule, 10).unwrap().len(), 3);
    assert_eq!(store.runs_for_rule(rule, 2).unwrap().len(), 2);
}

#[test]
fn a_rule_cannot_reference_a_volume_that_does_not_exist() {
    let store = Store::open_in_memory().unwrap();
    let mut spec = sample_spec(VolumeId(999));
    spec.name = "orphan".to_owned();
    assert!(
        store.create_rule(&spec, 1).is_err(),
        "the foreign key on source_volume must reject this"
    );
}

#[test]
fn the_same_destination_cannot_be_added_to_a_rule_twice() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    let rule = store.create_rule(&sample_spec(vol), 1).unwrap();
    let path = VolumePath {
        volume: vol,
        relative: PathBuf::from("Backups"),
    };

    store.add_destination(rule, &path, 0).unwrap();
    // Two identical destinations on one rule would race to write the same
    // files.
    assert!(store.add_destination(rule, &path, 1).is_err());
}

#[test]
fn a_half_written_retention_pair_is_rejected_by_the_schema() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    store.create_rule(&sample_spec(vol), 1).unwrap();

    // A kind without a count would read back as Unlimited and silently stop
    // pruning, so the constraint rejects it at the database level.
    let bad = store.conn.execute(
        "UPDATE rule SET retention_kind = 'keep_last_n', retention_value = NULL",
        [],
    );
    assert!(bad.is_err());
}

#[test]
fn unknown_enum_values_in_the_database_surface_as_errors() {
    let store = Store::open_in_memory().unwrap();
    let vol = volume_id(&store, r"\\?\Volume{1111}\");
    let id = store.create_rule(&sample_spec(vol), 1).unwrap();

    // Bypass the CHECK constraint to simulate a row written by a newer build.
    store
        .conn
        .execute_batch("PRAGMA writable_schema = ON;")
        .unwrap();
    let forced = store.conn.execute(
        "UPDATE rule SET schedule = 'fortnightly' WHERE id = ?1",
        [id],
    );
    store
        .conn
        .execute_batch("PRAGMA writable_schema = OFF;")
        .unwrap();

    if forced.is_ok() {
        // Reading must fail loudly rather than fall back to a default that
        // would change when the rule runs.
        assert!(store.rule(id).is_err());
    }
}

#[test]
fn deleting_a_missing_row_reports_not_found() {
    let store = Store::open_in_memory().unwrap();
    assert!(matches!(
        store.delete_rule(RuleId(42)),
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        store.delete_tag(TagId(42)),
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        store.delete_destination(DestinationId(42)),
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        store.rule(RuleId(42)),
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn a_database_survives_being_closed_and_reopened() {
    let dir = std::env::temp_dir().join(format!("shelv-store-test-{}", std::process::id()));
    let path = dir.join("nested").join(DB_FILENAME);
    let _ = std::fs::remove_dir_all(&dir);

    let id = {
        // Nested directories are created on open.
        let store = Store::open(&path).unwrap();
        let vol = volume_id(&store, r"\\?\Volume{persist}\");
        store.create_rule(&sample_spec(vol), 7).unwrap()
    };

    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), LATEST_VERSION);
    let rule = reopened.rule(id).unwrap();
    assert_eq!(rule.spec.name, "Lightroom Catalog");
    assert_eq!(rule.created_at, 7);

    drop(reopened);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unrecognised_drive_type_reads_back_as_unknown() {
    // Unknown is refused by DriveType::is_permitted, so a row this build does
    // not understand fails towards refusing to write to it.
    assert_eq!(drive_type_from_str("holographic"), DriveType::Unknown);
    assert!(!drive_type_from_str("holographic").is_permitted());

    for t in [
        DriveType::Fixed,
        DriveType::Removable,
        DriveType::Network,
        DriveType::Optical,
        DriveType::RamDisk,
        DriveType::Unknown,
    ] {
        assert_eq!(drive_type_from_str(drive_type_str(t)), t);
    }
}

#[test]
fn a_nickname_survives_everything_the_operating_system_writes() {
    // The property the whole feature rests on. `refresh_volume` runs once a
    // second off what the OS reports; if it, or a re-pick through
    // `upsert_volume`, wrote this column, the name the user chose would be
    // gone by the next tick.
    let store = Store::open_in_memory().unwrap();
    let id = volume_id(&store, "vol-a");

    store.set_volume_nickname(id, Some("Archive 4TB")).unwrap();

    // The drive is renamed in Explorer and reappears at another letter.
    let mut renamed = volume_info("vol-a", "G:\\");
    renamed.label = Some("Expansion".to_owned());
    store.refresh_volume(id, &renamed, 1_700_000_100).unwrap();
    // And the user picks another folder on it.
    store.upsert_volume(&renamed, Some(1_700_000_200)).unwrap();

    let read = store.volume(id).unwrap();
    assert_eq!(read.nickname.as_deref(), Some("Archive 4TB"));
    assert_eq!(
        read.label.as_deref(),
        Some("Expansion"),
        "the label still follows the OS"
    );
}

#[test]
fn a_blank_nickname_clears_rather_than_storing_an_empty_name() {
    // Otherwise the UI shows a blank where a name should be, instead of
    // falling back to the label.
    let store = Store::open_in_memory().unwrap();
    let id = volume_id(&store, "vol-a");

    store.set_volume_nickname(id, Some("Archive")).unwrap();
    store.set_volume_nickname(id, Some("   ")).unwrap();
    assert_eq!(store.volume(id).unwrap().nickname, None);

    store.set_volume_nickname(id, Some("Archive")).unwrap();
    store.set_volume_nickname(id, None).unwrap();
    assert_eq!(store.volume(id).unwrap().nickname, None);
}

#[test]
fn a_nickname_is_trimmed_rather_than_stored_with_its_whitespace() {
    let store = Store::open_in_memory().unwrap();
    let id = volume_id(&store, "vol-a");
    store
        .set_volume_nickname(id, Some("  Archive 4TB "))
        .unwrap();
    assert_eq!(
        store.volume(id).unwrap().nickname.as_deref(),
        Some("Archive 4TB")
    );
}

#[test]
fn forgetting_a_drive_a_rule_still_uses_is_refused_with_a_count() {
    // Cascading here would delete backup rules as a side effect of tidying a
    // drive list, which is not what pressing a button on a drive means.
    let store = Store::open_in_memory().unwrap();
    let source = volume_id(&store, "vol-source");
    let destination = volume_id(&store, "vol-destination");

    let rule = store.create_rule(&sample_spec(source), 1000).unwrap();
    store
        .add_destination(
            rule,
            &VolumePath {
                volume: destination,
                relative: PathBuf::from("Backups"),
            },
            0,
        )
        .unwrap();

    assert_eq!(store.volume_references(source).unwrap(), 1);
    assert_eq!(store.volume_references(destination).unwrap(), 1);

    let error = store
        .delete_volume(destination)
        .expect_err("a drive in use must not be forgotten");
    assert!(matches!(error, CoreError::Refused(_)), "{error:?}");
    assert!(format!("{error}").contains("1 rule"), "{error}");
    assert_eq!(store.volumes().unwrap().len(), 2, "nothing was removed");
}

#[test]
fn a_drive_used_twice_by_one_rule_still_counts_as_one_rule() {
    // A rule copying one folder on a drive to another folder on the same
    // drive touches it twice, and a rule with two destinations on one drive
    // touches it three times. Adding those up answers a question nobody
    // asked: the drives pane says "Rules", and one rule is one rule.
    let store = Store::open_in_memory().unwrap();
    let only = volume_id(&store, "vol-only");

    let rule = store.create_rule(&sample_spec(only), 1000).unwrap();
    for folder in ["Backups", "Backups-2"] {
        store
            .add_destination(
                rule,
                &VolumePath {
                    volume: only,
                    relative: PathBuf::from(folder),
                },
                0,
            )
            .unwrap();
    }

    assert_eq!(store.volume_references(only).unwrap(), 1);

    // And a second rule on the same drive does count.
    let second = store.create_rule(&sample_spec(only), 2000).unwrap();
    store
        .add_destination(
            second,
            &VolumePath {
                volume: only,
                relative: PathBuf::from("Elsewhere"),
            },
            0,
        )
        .unwrap();
    assert_eq!(store.volume_references(only).unwrap(), 2);
}

#[test]
fn an_unused_drive_can_be_forgotten() {
    let store = Store::open_in_memory().unwrap();
    let id = volume_id(&store, "vol-a");
    assert_eq!(store.volume_references(id).unwrap(), 0);

    store.delete_volume(id).unwrap();
    assert!(store.volumes().unwrap().is_empty());
}

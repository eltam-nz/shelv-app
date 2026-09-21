//! The whole path a rule travels, exercised together.
//!
//! The unit tests cover each piece in isolation. This covers the seams: a
//! folder is picked, which registers a volume, which lets a rule be created,
//! which the table then renders with the right availability — and the same
//! again for editing and deleting. Those seams are where the M2 work could
//! break without any single unit test noticing.
//!
//! It uses a stub platform rather than real drives so it is deterministic
//! everywhere, including CI. The real platform layer has its own tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a panic in a test is the failure report"
)]

use std::path::{Path, PathBuf};

use shelv_core::model::{
    Layout, Packaging, PlaceholderPolicy, Retention, RuleSpec, Schedule, VolumePath,
};
use shelv_core::platform::{
    CaseSensitivity, DriveType, PlatformFs, SpaceInfo, VolumeIdentity, VolumeIdentityKind,
    VolumeInfo,
};
use shelv_core::safety::{check_rule, RuleProblem};
use shelv_core::store::Store;
use shelv_core::view::{drive_rows, refresh_stored_volumes, rule_rows, Availability};
use shelv_core::volumes::resolve_picked_folder;
use shelv_core::{CoreError, Result};

/// A platform whose attached volumes the test controls.
struct StubFs {
    volumes: Vec<VolumeInfo>,
}

impl PlatformFs for StubFs {
    fn volumes(&self) -> Result<Vec<VolumeInfo>> {
        Ok(self.volumes.clone())
    }

    fn volume_of(&self, path: &Path) -> Result<VolumeInfo> {
        self.volumes
            .iter()
            .filter(|v| path.starts_with(&v.mount_point))
            .max_by_key(|v| v.mount_point.as_os_str().len())
            .cloned()
            .ok_or_else(|| CoreError::VolumeUnavailable("no volume contains that path".into()))
    }

    fn space(&self, _path: &Path) -> Result<SpaceInfo> {
        Ok(SpaceInfo {
            available: 1 << 30,
            total: 1 << 40,
        })
    }

    fn long_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn data_dir(&self) -> Result<PathBuf> {
        Ok(PathBuf::from("/tmp"))
    }

    fn trash(&self, _path: &Path) -> Result<()> {
        Ok(())
    }
}

fn volume(mount: &str, label: &str, drive_type: DriveType) -> VolumeInfo {
    VolumeInfo {
        identity: VolumeIdentity {
            kind: VolumeIdentityKind::LinuxFsUuid,
            value: format!("uuid-{label}"),
        },
        mount_point: PathBuf::from(mount),
        serial: None,
        label: Some(label.to_owned()),
        filesystem: Some("ext4".to_owned()),
        drive_type,
        case_sensitivity: CaseSensitivity::Sensitive,
        is_sync_root: false,
    }
}

fn system_and_backup() -> StubFs {
    StubFs {
        volumes: vec![
            volume("/", "System", DriveType::Fixed),
            volume("/media/backup", "Backup Drive", DriveType::Removable),
        ],
    }
}

fn spec(name: &str, source: VolumePath) -> RuleSpec {
    RuleSpec {
        name: name.to_owned(),
        enabled: true,
        source,
        layout: Layout::Mirror,
        packaging: Packaging::Files,
        retention: Retention::Unlimited,
        schedule: Schedule::Weekly,
        run_on_connect: true,
        catch_up: true,
        placeholders: PlaceholderPolicy::Hydrate,
        hydrate_budget_bytes: None,
        follow_symlinks: false,
        excludes: Vec::new(),
    }
}

#[test]
fn a_rule_can_be_created_edited_and_deleted_through_the_core() {
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    // Nothing exists until a folder is picked. This is the state a fresh
    // install is in, and the reason the picker is the entry point.
    assert!(store.volumes().unwrap().is_empty());
    assert!(rule_rows(&store, &fs).unwrap().is_empty());

    // Pick a source and a destination. Each registers its volume.
    let source =
        resolve_picked_folder(&store, &fs, Path::new("/root/Pictures/Lightroom"), 1000).unwrap();
    let destination =
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 1000).unwrap();

    assert_eq!(store.volumes().unwrap().len(), 2);
    assert_eq!(
        source.path.relative,
        PathBuf::from("root/Pictures/Lightroom")
    );
    assert_eq!(destination.path.relative, PathBuf::from("Backups"));

    // Create it.
    let rule_spec = spec("Lightroom Catalog", source.path.clone());
    assert_eq!(
        check_rule(
            &rule_spec,
            std::slice::from_ref(&destination.path),
            None,
            &[None],
            false
        ),
        Vec::new(),
        "this rule should pass the guards"
    );
    let rule = store.create_rule(&rule_spec, 1000).unwrap();
    store.add_destination(rule, &destination.path, 0).unwrap();

    let tag = store.create_tag("images", "pastel-blue").unwrap();
    store.set_rule_tags(rule, &[tag]).unwrap();

    // It renders as one row, with both volumes attached.
    let rows = rule_rows(&store, &fs).unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows.first().unwrap();
    assert_eq!(row.rule.spec.name, "Lightroom Catalog");
    assert_eq!(row.tags.len(), 1);
    assert_eq!(row.source.availability, Availability::Available);
    assert_eq!(row.destinations.len(), 1);
    assert_eq!(
        row.destinations.first().unwrap().status.availability,
        Availability::Available
    );
    assert!(row.is_runnable());
    assert!(row.last_run.is_none(), "it has never run");

    // Edit it.
    let mut changed = rule_spec.clone();
    changed.name = "Lightroom Catalog (weekly)".to_owned();
    changed.layout = Layout::Snapshot;
    changed.retention = Retention::KeepLastN(6);
    store.update_rule(rule, &changed).unwrap();

    let row = rule_rows(&store, &fs).unwrap().into_iter().next().unwrap();
    assert_eq!(row.rule.spec.name, "Lightroom Catalog (weekly)");
    assert_eq!(row.rule.spec.layout, Layout::Snapshot);
    assert_eq!(row.tags.len(), 1, "editing must not drop the tags");
    assert_eq!(row.destinations.len(), 1, "or the destinations");

    // Delete it. The volumes and the tag outlive it.
    store.delete_rule(rule).unwrap();
    assert!(rule_rows(&store, &fs).unwrap().is_empty());
    assert_eq!(store.volumes().unwrap().len(), 2);
    assert_eq!(store.tags().unwrap().len(), 1);
}

#[test]
fn unplugging_the_destination_drive_makes_the_rule_unrunnable_but_keeps_it() {
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    let source = resolve_picked_folder(&store, &fs, Path::new("/root/Pictures"), 1000).unwrap();
    let destination =
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 1000).unwrap();

    let rule = store
        .create_rule(&spec("Photos", source.path), 1000)
        .unwrap();
    store.add_destination(rule, &destination.path, 0).unwrap();

    // Unplug the backup drive. The rule must survive — configuring against a
    // drive that is only occasionally connected is the whole point.
    let unplugged = StubFs {
        volumes: vec![volume("/", "System", DriveType::Fixed)],
    };

    let row = rule_rows(&store, &unplugged)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        row.destinations.first().unwrap().status.availability,
        Availability::Disconnected
    );
    assert!(!row.is_runnable());
    assert_eq!(row.rule.spec.name, "Photos", "the rule is still there");
}

#[test]
fn a_different_drive_in_the_same_slot_is_reported_as_a_mismatch() {
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    let source = resolve_picked_folder(&store, &fs, Path::new("/root/Pictures"), 1000).unwrap();
    let destination =
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 1000).unwrap();
    let rule = store
        .create_rule(&spec("Photos", source.path), 1000)
        .unwrap();
    store.add_destination(rule, &destination.path, 0).unwrap();

    // Someone plugs a *different* disk into the same mount point. Treating
    // this as available is what would write the backup onto a stranger's
    // drive, so it must be called out as its own state.
    let mut impostor = volume("/media/backup", "Not Yours", DriveType::Removable);
    impostor.identity.value = "uuid-someone-else".to_owned();
    let swapped = StubFs {
        volumes: vec![volume("/", "System", DriveType::Fixed), impostor],
    };

    let row = rule_rows(&store, &swapped)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        row.destinations.first().unwrap().status.availability,
        Availability::IdentityMismatch
    );
    assert!(!row.is_runnable());
}

#[test]
fn a_rule_that_would_swallow_its_own_output_is_refused_at_creation() {
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    let source = resolve_picked_folder(&store, &fs, Path::new("/root/Pictures"), 1000).unwrap();
    let inside =
        resolve_picked_folder(&store, &fs, Path::new("/root/Pictures/Backups"), 1000).unwrap();

    let problems = check_rule(
        &spec("Recursive", source.path),
        std::slice::from_ref(&inside.path),
        None,
        &[None],
        false,
    );
    assert_eq!(
        problems,
        vec![RuleProblem::DestinationInsideSource {
            destination_index: 0
        }]
    );
}

#[test]
fn picking_a_folder_on_a_refused_drive_never_reaches_rule_creation() {
    let store = Store::open_in_memory().unwrap();
    let fs = StubFs {
        volumes: vec![
            volume("/", "System", DriveType::Fixed),
            volume("/mnt/nas", "NAS", DriveType::Network),
        ],
    };

    let error = resolve_picked_folder(&store, &fs, Path::new("/mnt/nas/Backups"), 1000)
        .expect_err("a network share must be refused at the picker");
    assert!(matches!(error, CoreError::Refused(_)), "{error:?}");

    // And it leaves nothing behind that a later rule could reference.
    assert!(store.volumes().unwrap().is_empty());
}

#[test]
fn renaming_a_drive_updates_the_stored_name_without_enrolling_new_drives() {
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    let source = resolve_picked_folder(&store, &fs, Path::new("/root/Pictures"), 1000).unwrap();
    let destination =
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 1000).unwrap();
    let rule = store
        .create_rule(&spec("Photos", source.path), 1000)
        .unwrap();
    store.add_destination(rule, &destination.path, 0).unwrap();
    assert_eq!(store.volumes().unwrap().len(), 2);

    // The user renames the backup drive, and plugs in a third drive they
    // have never picked a folder on.
    let mut renamed = volume("/media/backup", "Backup Drive", DriveType::Removable);
    renamed.label = Some("Archive 2026".to_owned());
    let stranger = volume("/media/usb", "Someone Else", DriveType::Removable);
    let after = StubFs {
        volumes: vec![volume("/", "System", DriveType::Fixed), renamed, stranger],
    };

    assert_eq!(refresh_stored_volumes(&store, &after, 2000).unwrap(), 1);

    // The rename is persisted...
    let stored_names: Vec<_> = store
        .volumes()
        .unwrap()
        .into_iter()
        .filter_map(|v| v.label)
        .collect();
    assert!(
        stored_names.contains(&"Archive 2026".to_owned()),
        "{stored_names:?}"
    );

    // ...and the drive nobody picked a folder on is still not in Shelv.
    // Attaching a drive must never be what enrols it.
    assert_eq!(store.volumes().unwrap().len(), 2);
    assert!(!stored_names.contains(&"Someone Else".to_owned()));

    // A second pass with nothing changed writes nothing.
    assert_eq!(refresh_stored_volumes(&store, &after, 3000).unwrap(), 0);

    // And the table shows the new name.
    let row = rule_rows(&store, &after)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        row.destinations
            .first()
            .unwrap()
            .status
            .volume
            .label
            .as_deref(),
        Some("Archive 2026")
    );
}

#[test]
fn the_drives_pane_reports_use_and_refuses_to_forget_a_drive_in_use() {
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    let source = resolve_picked_folder(&store, &fs, Path::new("/root/Pictures"), 1000).unwrap();
    let destination =
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 1000).unwrap();
    let rule = store
        .create_rule(&spec("Photos", source.path), 1000)
        .unwrap();
    store.add_destination(rule, &destination.path, 0).unwrap();

    // The user names the backup drive.
    store
        .set_volume_nickname(destination.path.volume, Some("Archive 4TB"))
        .unwrap();

    let rows = drive_rows(&store, &fs).unwrap();
    assert_eq!(rows.len(), 2, "both drives the rule uses");
    let backup = rows
        .iter()
        .find(|r| r.status.volume.id == destination.path.volume)
        .expect("the backup drive");
    assert_eq!(
        backup.status.volume.nickname.as_deref(),
        Some("Archive 4TB")
    );
    assert_eq!(backup.status.availability, Availability::Available);
    assert_eq!(backup.rule_count, 1);

    // Forgetting it is refused while the rule points at it...
    let error = store
        .delete_volume(destination.path.volume)
        .expect_err("a drive in use must not be forgotten");
    assert!(matches!(error, CoreError::Refused(_)), "{error:?}");

    // ...and allowed once nothing does.
    store.delete_rule(rule).unwrap();
    store.delete_volume(destination.path.volume).unwrap();
    assert_eq!(drive_rows(&store, &fs).unwrap().len(), 1);
}

#[test]
fn a_nickname_outlives_the_drive_being_unplugged_and_renamed() {
    // The case the nickname exists for: the drive goes in a drawer, comes
    // back at a different letter with a different label, and is still the
    // one the user named.
    let store = Store::open_in_memory().unwrap();
    let fs = system_and_backup();

    let picked =
        resolve_picked_folder(&store, &fs, Path::new("/media/backup/Backups"), 1000).unwrap();
    store
        .set_volume_nickname(picked.path.volume, Some("Archive 4TB"))
        .unwrap();

    // Unplugged.
    let unplugged = StubFs {
        volumes: vec![volume("/", "System", DriveType::Fixed)],
    };
    refresh_stored_volumes(&store, &unplugged, 2000).unwrap();
    let row = drive_rows(&store, &unplugged)
        .unwrap()
        .into_iter()
        .find(|r| r.status.volume.id == picked.path.volume)
        .expect("still recorded");
    assert_eq!(row.status.availability, Availability::Disconnected);
    assert_eq!(row.status.volume.nickname.as_deref(), Some("Archive 4TB"));

    // Back, relabelled and at a new mount point.
    let mut moved = volume("/media/usb0", "Renamed In Explorer", DriveType::Removable);
    moved.identity.value = "uuid-Backup Drive".to_owned();
    let back = StubFs {
        volumes: vec![volume("/", "System", DriveType::Fixed), moved],
    };
    refresh_stored_volumes(&store, &back, 3000).unwrap();

    let row = drive_rows(&store, &back)
        .unwrap()
        .into_iter()
        .find(|r| r.status.volume.id == picked.path.volume)
        .expect("recognised by identity, not by letter");
    assert_eq!(row.status.availability, Availability::Available);
    assert_eq!(row.status.volume.nickname.as_deref(), Some("Archive 4TB"));
    assert_eq!(
        row.status.volume.label.as_deref(),
        Some("Renamed In Explorer"),
        "the label follows the OS; the nickname does not"
    );
}

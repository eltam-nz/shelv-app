//! Seeds a database with one rule, for looking at the app on a machine that
//! has no drives Shelv would accept.
//!
//! Development only. It writes a volume row with a stable-looking identity
//! so a rule can exist at all: in a container there is no `/dev/disk/by-uuid`,
//! so every real volume reads as `Unverified` and the guards refuse — which
//! is correct, and also means the table cannot be seen with anything in it.
//!
//! Usage: `cargo run -p shelv-core --example seed -- <db> <source> <dest>`

use std::path::{Path, PathBuf};

use shelv_core::model::{
    Layout, Packaging, PlaceholderPolicy, Retention, RuleSpec, Schedule, VolumePath,
};
use shelv_core::platform::{
    CaseSensitivity, DriveType, VolumeIdentity, VolumeIdentityKind, VolumeInfo,
};
use shelv_core::store::Store;

fn main() -> shelv_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, db, source, destination] = args.as_slice() else {
        eprintln!("usage: seed <db> <source-dir> <destination-dir>");
        std::process::exit(2);
    };

    let store = Store::open(Path::new(db))?;
    let source_volume = store.upsert_volume(&volume(source, "seed-source"), Some(0))?;
    let dest_volume = store.upsert_volume(&volume(destination, "seed-destination"), Some(0))?;

    let rule = store.create_rule(
        &RuleSpec {
            name: "Pictures".to_owned(),
            enabled: true,
            source: VolumePath {
                volume: source_volume,
                relative: PathBuf::new(),
            },
            layout: Layout::Mirror,
            packaging: Packaging::Files,
            retention: Retention::Unlimited,
            schedule: Schedule::Manual,
            run_on_connect: false,
            placeholders: PlaceholderPolicy::Hydrate,
            hydrate_budget_bytes: None,
            follow_symlinks: false,
            excludes: Vec::new(),
        },
        0,
    )?;
    store.add_destination(
        rule,
        &VolumePath {
            volume: dest_volume,
            relative: PathBuf::new(),
        },
        0,
    )?;

    println!("seeded {db}");
    Ok(())
}

fn volume(mount: &str, id: &str) -> VolumeInfo {
    VolumeInfo {
        identity: VolumeIdentity {
            kind: VolumeIdentityKind::LinuxFsUuid,
            value: id.to_owned(),
        },
        mount_point: PathBuf::from(mount),
        serial: None,
        label: Some(id.to_owned()),
        filesystem: Some("ext4".to_owned()),
        drive_type: DriveType::Removable,
        case_sensitivity: CaseSensitivity::Sensitive,
        is_sync_root: false,
    }
}

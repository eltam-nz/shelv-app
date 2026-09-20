//! Prints the volumes Shelv can see, and how it classifies each one.
//!
//! A debugging aid for "why is my drive showing as unavailable": run it with
//! the drive plugged in and unplugged and compare.
//!
//! Run: cargo run -p shelv-core --example volumes

fn main() {
    let fs = shelv_core::platform::host_fs();
    match fs.volumes() {
        Ok(volumes) => {
            for v in volumes {
                println!(
                    "{:<24} {:<12} {:<8} permitted={:<5} identity={}",
                    v.mount_point.display(),
                    v.filesystem.as_deref().unwrap_or("-"),
                    format!("{:?}", v.drive_type),
                    v.drive_type.is_permitted(),
                    v.identity.value,
                );
            }
        }
        Err(e) => eprintln!("could not enumerate volumes: {e}"),
    }
}

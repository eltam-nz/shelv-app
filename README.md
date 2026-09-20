# Shelv

A local backup manager for Windows: define rules that copy folders from your
computer to internal disks, external drives and your local OneDrive folder, on
a schedule or when a drive is plugged in.

**Status: planning.** No code yet. The design proposal is in
[`docs/PLAN.md`](docs/PLAN.md) and is open for review.

## Intended scope

- Backup **rules**: a source folder and one or more destination folders.
- Backup **layouts**: mirror in place, or timestamped snapshot subfolders with retention.
- Optional **lossless** `.zip` packaging (Zip64, store or deflate).
- **Schedules**: daily / weekly / monthly / manual, plus catch-up for missed runs
  and automatic runs when a destination drive is connected.
- Clear **availability** state for destinations on drives that are not currently plugged in.
- Colour-coded **tags** for sorting and filtering rules.
- Runs in the background from the system tray if you want it to.

## Deliberately not in scope

Cloud or network destinations of any kind, and therefore no accounts, no OAuth
and no stored credentials. Shelv reads the local OneDrive folder on disk; it
never talks to the Microsoft Graph API. It makes no network requests except to
check for its own updates, and that can be turned off.

See [`docs/PLAN.md`](docs/PLAN.md) §4 for the full threat model.

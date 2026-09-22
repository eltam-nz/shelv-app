# Shelv

A local backup manager for Windows: define rules that copy folders from your
computer to internal disks, external drives and your local OneDrive folder, on
a schedule or when a drive is plugged in.

**Status: early development.** M0 (the skeleton), M2 (rule management) and M1
(the backup engine) are complete: Shelv copies files, mirrors deletions into a
recoverable trash folder, writes timestamped snapshots, and runs from Backup
Now with progress and a preview of anything it would remove. Scheduling,
OneDrive hydration and `.zip` packaging are still to come — a rule set to
write an archive is refused rather than quietly copying loose files.

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — how it is built
- [`docs/SECURITY.md`](docs/SECURITY.md) — threat model and what is enforced
- [`docs/PLAN.md`](docs/PLAN.md) — the original proposal and its reasoning
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to build it and what CI checks

## Scope

- Backup **rules**: a source folder and one or more destination folders.
- Backup **layouts**: mirror in place, or timestamped snapshot subfolders with retention.
- Optional **lossless** `.zip` packaging (Zip64, store or deflate).
- **Schedules**: daily / weekly / monthly / manual, plus catch-up for missed runs
  and automatic runs when a destination drive is connected.
- Clear **availability** state for destinations on drives that are not plugged in.
- Colour-coded **tags** for sorting and filtering rules.
- **Dry run** preview, exclude patterns, preflight free-space checks and per-run history.
- Runs in the background from the system tray.

## Trying it

Every push builds a Windows executable. To get one:

1. Open the [Actions tab](https://github.com/eltam-nz/shelv-app/actions), pick
   the most recent run on `main`, and scroll to **Artifacts**.
2. Download `shelv-windows-x64-…` and unzip it.
3. Run `shelv.exe`. There is nothing to install — the whole app is that one
   file, and deleting it removes it.

Windows SmartScreen will warn on first run, because the build is unsigned.
*More info → Run anyway*. See [`docs/PLAN.md`](docs/PLAN.md) §3.4 for why, and
what signing would cost.

Shelv keeps its database in `%LOCALAPPDATA%\Shelv`. Deleting that folder
resets it completely.

### What you can do today

Create backup rules, choose folders through the native picker, tag them, edit
and delete them. Rules are checked as you write them: a destination inside its
own source, a drive Shelv will not write to, a missing name.

**Nothing is copied yet.** The backup engine is the next milestone, so
*Backup Now* is disabled everywhere and the status bar says so. That also
means running it is safe in the strongest sense — there is no code in it that
writes to, deletes or moves any file outside its own database.

To build from source instead, see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## OneDrive

Shelv backs up cloud-only OneDrive files. Where a file is a Files On-Demand
placeholder, Shelv asks Windows to download it, copies it, and can then release
the local copy again to reclaim the space — releasing only files it downloaded
itself, never ones you had pinned. Hydration is bounded by a per-rule budget and
a preflight free-space check, so a large cloud library cannot fill your system
drive.

This works through the local OneDrive client. Shelv never talks to the Microsoft
Graph API, so there are no accounts, no OAuth and no stored credentials.

## Platform

Windows-only for v1. Linux is a future target, so all platform-specific code
sits behind an abstraction and CI builds on Linux from the start.

## Privacy

No telemetry, no analytics, no crash reporting. The only network request Shelv
makes is an update check against GitHub, and it can be turned off.

See [`docs/SECURITY.md`](docs/SECURITY.md) for the full threat model.

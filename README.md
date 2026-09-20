# Shelv

A local backup manager for Windows: define rules that copy folders from your
computer to internal disks, external drives and your local OneDrive folder, on
a schedule or when a drive is plugged in.

**Status: early development.** Milestone M0 — the skeleton — is complete: the
workspace, storage, platform abstraction, IPC surface, theme and rule table are
in place, with CI on Windows and Linux. The backup engine itself lands in M1,
so nothing is copied yet.

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

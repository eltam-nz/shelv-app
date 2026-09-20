# Shelv — Design & Build Plan

> Status: approved for implementation. Revision 2.
> Scope: feature review (§1), architecture (§2), repo/build/install (§3), security (§4), roadmap (§5).

## Decisions taken

| #                 | Decision                                                                                                                                              |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| Name              | **Shelv**                                                                                                                                             |
| Stack             | **Tauri 2 — Rust core, React + TypeScript frontend**                                                                                                  |
| Platform          | **Windows-only for v1.** Linux is an explicit future target, so every platform-specific decision is made behind an abstraction from commit one (§2.6) |
| Code signing      | **Ship unsigned for v1.** SmartScreen warning documented in the README                                                                                |
| Repository        | **Private for now**, possibly public later — so nothing in the code or history may assume privacy (§4.4)                                              |
| Verification mode | **Deferred to v1.1.** Not in v1 scope                                                                                                                 |

All other recommended features from revision 1 are accepted into v1 scope.

---

## 1. Feature review

### 1.1 Corrections — problems the original spec will hit

**a) Drive letters are not stable identifiers.**
Windows assigns letters from a registry mapping; unplug two drives, reconnect them in a different order, and `E:` can be a different physical disk. A rule keyed on a letter can write onto the wrong drive, and a mirror-with-deletions rule onto the wrong drive is data loss.

Fix: store each volume as an opaque, platform-specific stable identity (§2.6) plus label, and resolve the current mount point at run time. Verify the identity matches before the first write. On mismatch the rule is UNAVAILABLE — never fall back to the letter.

**b) "Lossless compression only" is already guaranteed — the real risks are different.**
Every general-purpose archive codec (Deflate, zstd, LZMA, bzip2) is lossless by construction; lossy compression exists only in media-specific codecs, which an archiver never applies. So requirement 4 is satisfied by any standard `.zip`. Two things do need handling:

- **Zip64.** The original ZIP format caps entries at 65,535 and both archive and member size at 4 GiB. A Lightroom export folder will exceed this. Zip64 must be explicitly enabled.
- **Compressing already-compressed data is near-pointless.** RAW, JPEG, PNG and MP4 are already entropy-coded; Deflate typically recovers a low single-digit percentage while costing full-throughput CPU, and it converts a resumable incremental copy into an all-or-nothing archive rewrite. Offer per-rule `None | Store | Deflate`, default **None** for image rules and **Deflate** for document rules, and display the achieved ratio after each run so the setting can be chosen on evidence.

**c) A timestamped-subfolder rule grows without bound.**
Retention is part of that backup style, not an extra: per-rule `keep last N` and/or `keep newer than N days`, pruning only after a _successful_ run.

**d) Frequency alone is the wrong trigger for a removable-drive rule.**
Four of six rules in the mock-up target an unavailable drive. A "monthly on the 1st" rule whose drive appears on the 3rd never runs. Two behaviours, both small:

- **Catch-up:** a missed run (machine off, drive absent) executes at the next opportunity rather than being skipped to the next period.
- **Run on connect:** when a volume a rule depends on appears, queue that rule. For occasionally-connected drives this is the _primary_ trigger; frequency becomes "don't run more often than".

**e) The table has no run _result_ column.**
"Last Backup: 14-Sept-26" does not say whether it worked. Add `OK / Partial (n skipped) / Failed / Never run` plus a per-run log.

**f) Filesystem differences between source and destination.**
External drives are often exFAT or FAT32:

- FAT32 stores mtime at 2-second granularity (exFAT 10 ms). Exact-equality mtime comparison re-copies every file every run. Use a 2-second tolerance.
- NTFS ACLs and alternate data streams do not survive to exFAT. Paths over 260 characters need the `\\?\` prefix or the copy fails — common in Lightroom trees.

**g) Overwrite mode must not mean "delete the destination first".**
Wipe-then-copy leaves no backup at all if interrupted. Mirror is an in-place incremental sync: copy changed files to a temp name, `rename` into place (atomic on the same volume), and remove extraneous files only when deletions are explicitly enabled.

### 1.2 OneDrive Files On-Demand — hydrate then copy

**Yes, Shelv can download cloud-only files and back them up, and this is now the default.** The mechanism, the constraints and the traps:

**How hydration is triggered.** Reading a placeholder through ordinary `CreateFile`/`ReadFile` — without `FILE_FLAG_OPEN_NO_RECALL` — makes Windows fetch the content from OneDrive automatically. Hydrating therefore requires _no special API_; a normal file copy already does it. The Cloud Files API is only needed to do the opposite (detect and skip), and to release space afterwards. This is the easy direction.

**Four constraints that do need engineering:**

1. **Shelv must not run as a Windows service.** Processes in a service context receive `STATUS_CLOUD_FILE_ACCESS_DENIED` (`0xC000CF18`) instead of hydration. Background operation is therefore implemented as a **per-user tray process**, never a service. This was already preferred for privilege hygiene (§4, T9); it is now a correctness requirement.

2. **Windows may ask the user for permission.** Automatic background downloads can raise an interactive toast, and if the user blocks Shelv it stays blocked until re-enabled under _Settings → Automatic file downloads_. Detect that specific denial and surface actionable guidance, not a generic I/O error.

3. **Disk space is the binding constraint.** Hydrating a 500 GB cloud library needs 500 GB of local free space, transiently. Three mitigations, all in v1 scope:
   - **Preflight:** estimate hydration bytes from the placeholders' logical sizes and check free space on _both_ the OneDrive volume and the destination before starting.
   - **Hydrate → copy → release, in bounded batches**, rather than hydrating the whole tree first. Peak local usage stays at roughly the batch size instead of the whole library.
   - **Per-rule hydration budget** — an explicit cap, above which the run stops and reports rather than filling the system disk.

4. **Releasing space afterwards.** `CfSetPinState(CF_PIN_STATE_UNPINNED)` — Microsoft documents that any application, not only the sync provider, may call it — or equivalently `attrib -p +u`. Two cautions: dehydration is a _request_, asynchronous and not guaranteed, so the budget logic must tolerate space being freed lazily; and Shelv must **record each file's pre-existing pin state and only release files it hydrated itself**, never unpinning something the user deliberately keeps offline-available.

**Two known platform bugs to defend against:**

- `CreateFileMapping` forces full hydration regardless of progressive-hydration policy — so use plain sequential reads and **never memory-mapped I/O** on a sync root.
- Hydration of files of exactly 4096 bytes has been reported to hang. Apply a per-file hydration timeout and record a timeout as a skipped file rather than letting one file block the run.

Hydration is network-bound, so parallelism for cloud sources is configured separately from, and lower than, local-disk parallelism.

Per-rule setting:

```
Cloud placeholders : Hydrate and back up          (default)
                   | Hydrate, back up, then release local copy
                   | Skip placeholders
```

Shelv never _writes_ into a sync root; OneDrive paths are read-only sources.

### 1.3 Accepted for v1

| Feature                               | Why                                                                                                                  |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| **Dry run / preview**                 | Shows files to copy, bytes and _deletions_ before anything is touched — the best guard against a mis-configured rule |
| **Exclude patterns (globs)**          | Skip `Thumbs.db`, `*.tmp`, `.lrcat-journal`, `.lrprev`; cuts a Lightroom backup substantially                        |
| **Preflight free-space check**        | Refuse a run that cannot fit, rather than filling the drive and failing halfway                                      |
| **Per-run history + log**             | Powers the result column in §1.1(e); required for any diagnosis                                                      |
| **Tray icon + desktop notifications** | Background operation; a background backup that fails silently is worse than none                                     |
| **Import/export rules as JSON**       | Config backup and machine migration                                                                                  |

### 1.4 Out of scope for v1

Declined deliberately: **verification mode** (size/mtime is used for the incremental diff; BLAKE3 content verification moves to v1.1), hard-linked snapshots, Volume Shadow Copy for locked files, restore-from-backup UI, cloud/remote or network destinations, encrypted archives, block-level delta sync, multi-user or server mode, mobile.

Note on locked files: Lightroom holds its catalog open, and without VSS a locked file cannot be copied. v1 detects the sharing violation, records the file as skipped and marks the run **Partial** — it never claims success on a torn copy. VSS is the v1.1 answer.

### 1.5 Revised feature model

The requested "styles" decompose into orthogonal axes, removing combinatorial special cases from both code and UI:

```
layout      : Mirror | Snapshot (timestamped subfolder)
packaging   : Files  | Zip (store | deflate)
retention   : Snapshot only — KeepLastN | KeepDays
deletions   : Mirror only — Off (default) | On
placeholders: Hydrate | HydrateAndRelease | Skip
```

`Backup Type: Overwrite` in the mock-up = `Mirror + Files + deletions off`.
`Backup Type: Subfolder` = `Snapshot + Files + KeepLastN`.

---

## 2. Architecture

### 2.1 Stack

**Tauri 2 — Rust core, React 19 + TypeScript frontend.** The workload is "walk millions of directory entries and copy bytes in parallel, behind a table-heavy UI". Rust handles the first half with `rayon`/`jwalk` and no GC pauses; a web frontend handles the second far faster than XAML. ~10 MB installer, native MSI and NSIS bundling, and a capability system that _enforces_ the §4 security constraint rather than relying on convention. Tauri 2 is on the 2.10.x line with active releases through 2026.

### 2.2 Components

Design rule: **the WebView never touches the filesystem.** All I/O lives in Rust behind typed commands that take rule IDs, not paths.

```
┌──────────────────────── WebView (React 19 + TS) ─────────────────────────┐
│  RuleTable (TanStack Table) · RuleEditor · TagManager · RunHistory       │
│  Zustand store  ·  no fs access  ·  strict CSP  ·  no eval               │
└───────────────┬──────────────────────────────▲──────────────────────────┘
      invoke()  │ typed commands (rule ids)    │ events: progress, volume,
                ▼                              │         run-complete
┌──────────────────────── Rust core (tokio) ───────────────────────────────┐
│  commands/     IPC surface; validates + authorises every call            │
│  engine/       plan → execute → prune                                    │
│      planner     walk, filter, diff (size + mtime ±2s), build work list  │
│      copier      parallel copy, temp+rename, long paths, retry           │
│      archiver    zip (Zip64), store | deflate                            │
│      retention   prune snapshots, only after a successful run            │
│  cloud/        placeholder detect, hydrate, batch budget, pin-state      │
│                restore, release                    [Windows only]        │
│  platform/     trait PlatformFs + windows.rs / linux.rs  (§2.6)          │
│  volumes/      enumerate, stable identity, drive-type gate, arrival      │
│  scheduler/    cron eval, catch-up, run-on-connect, run queue + locks    │
│  store/        SQLite (rusqlite) — rules, tags, volumes, runs, events    │
│  safety/       canonicalisation, allowlist, destructive-op guards        │
└───────────────┬──────────────────────────────────────────────────────────┘
                ▼
         filesystem  ·  local disks, external volumes, local OneDrive folder
```

**Concurrency.** One `tokio` runtime. The scheduler owns a run queue; at most one run per destination volume (a per-volume mutex) so two rules cannot interleave writes on one disk. Within a run, walking and copying use a bounded worker pool — around 4 for mechanical disks, higher for SSDs, and separately (lower) for cloud-hydrating sources. Oversubscribing a mechanical external drive makes it slower, not faster. Progress is streamed to the UI as events throttled to ≤10/s so a million-file run does not flood the IPC channel.

**Cancellation.** Every run carries a cancellation token checked between files. Temp+rename means a cancelled run leaves no partial file behind.

### 2.3 Data model (SQLite via `rusqlite`, bundled)

```sql
CREATE TABLE volume (
  id             INTEGER PRIMARY KEY,
  identity_kind  TEXT NOT NULL,          -- 'win_volume_guid' | 'linux_fs_uuid'
  identity       TEXT NOT NULL,          -- opaque, platform-specific (§2.6)
  serial         TEXT,
  label          TEXT,
  filesystem     TEXT,                   -- NTFS | exFAT | FAT32 | ext4 …
  drive_type     TEXT NOT NULL,          -- fixed | removable  (network rejected)
  is_sync_root   INTEGER NOT NULL DEFAULT 0,
  last_seen_at   INTEGER,
  last_mount     TEXT,                   -- display only, never used to resolve
  UNIQUE (identity_kind, identity)
);

CREATE TABLE rule (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL,
  enabled         INTEGER NOT NULL DEFAULT 1,
  source_volume   INTEGER NOT NULL REFERENCES volume(id),
  source_rel      TEXT NOT NULL,
  layout          TEXT NOT NULL,         -- mirror | snapshot
  packaging       TEXT NOT NULL,         -- files | zip_store | zip_deflate
  allow_deletions INTEGER NOT NULL DEFAULT 0,
  retention_kind  TEXT,                  -- keep_last_n | keep_days | null
  retention_value INTEGER,
  schedule        TEXT NOT NULL,         -- manual | daily | weekly | monthly | cron:<expr>
  run_on_connect  INTEGER NOT NULL DEFAULT 1,
  catch_up        INTEGER NOT NULL DEFAULT 1,
  placeholders    TEXT NOT NULL DEFAULT 'hydrate',  -- hydrate | hydrate_release | skip
  hydrate_budget_bytes INTEGER,          -- null = unlimited
  follow_symlinks INTEGER NOT NULL DEFAULT 0,
  excludes        TEXT,                  -- JSON array of globs
  created_at      INTEGER NOT NULL
);
-- v1.1 migration adds: verify TEXT NOT NULL DEFAULT 'size_mtime'

CREATE TABLE destination (          -- a rule may have several ("add backup directories")
  id         INTEGER PRIMARY KEY,
  rule_id    INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
  volume_id  INTEGER NOT NULL REFERENCES volume(id),
  dest_rel   TEXT NOT NULL,
  sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE tag (
  id     INTEGER PRIMARY KEY,
  name   TEXT NOT NULL UNIQUE,
  colour TEXT NOT NULL                   -- from a curated, contrast-checked pastel set
);
CREATE TABLE rule_tag (
  rule_id INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
  tag_id  INTEGER NOT NULL REFERENCES tag(id)  ON DELETE CASCADE,
  PRIMARY KEY (rule_id, tag_id)
);

CREATE TABLE run (
  id             INTEGER PRIMARY KEY,
  rule_id        INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
  destination_id INTEGER NOT NULL REFERENCES destination(id) ON DELETE CASCADE,
  trigger        TEXT NOT NULL,          -- manual | schedule | catch_up | on_connect
  started_at     INTEGER NOT NULL,
  finished_at    INTEGER,
  result         TEXT,                   -- ok | partial | failed | cancelled
  files_copied   INTEGER DEFAULT 0,
  files_skipped  INTEGER DEFAULT 0,
  files_deleted  INTEGER DEFAULT 0,
  bytes_copied   INTEGER DEFAULT 0,
  compressed_bytes     INTEGER,          -- powers the "was compression worth it" readout
  placeholders_hydrated INTEGER DEFAULT 0,
  placeholders_skipped  INTEGER DEFAULT 0,
  bytes_hydrated        INTEGER DEFAULT 0,
  bytes_released        INTEGER DEFAULT 0,
  error          TEXT,
  snapshot_path  TEXT
);

CREATE TABLE run_event (                 -- per-file problems; capped and rotated
  id      INTEGER PRIMARY KEY,
  run_id  INTEGER NOT NULL REFERENCES run(id) ON DELETE CASCADE,
  level   TEXT NOT NULL,                 -- warn | error
  path    TEXT NOT NULL,
  message TEXT NOT NULL
);
```

Migrations via `user_version` steps or `refinery`; either is adequate at this size.

### 2.4 Key Rust dependencies

`tauri` 2 · `rusqlite` (bundled SQLite) · `tokio` · `serde`/`serde_json` · `jwalk` (parallel traversal) · `rayon` · `zip` (Zip64) · `globset` · `croner` · `chrono` · `windows` (Win32: `GetVolumeInformationW`, `GetDriveTypeW`, `FindFirstVolumeW`, `CfSetPinState`, file attributes) · `tracing` + `tracing-subscriber` · `thiserror`/`anyhow` · `sysinfo`.

Tauri plugins: `autostart`, `notification`, `updater`, `single-instance`, `dialog`, `log`, `window-state`.

### 2.5 Frontend

React 19 + TypeScript + Vite. **TanStack Table v8** for the rule table (sorting, per-tag filtering, column sizing — exactly the mock-up's shape). Tailwind v4. Zustand for client state; Tauri events push server state.

**Theme.** Dark neutral base with pastel accents. Define the palette as CSS custom properties on `:root` so tag and status colours come from one place. Two hard constraints: status is never conveyed by colour alone (AVAILABLE/UNAVAILABLE gets an icon and text), and pastel-on-dark must still clear WCAG AA (4.5:1) for text — pastels fail this easily, so the tag palette is contrast-checked once and fixed rather than picked per tag.

**Columns.** Tags · Name · Source · Destination · Availability (with volume label) · Layout · Compression · Schedule · **Last Result** · Last Run · Next Run · Actions.

### 2.6 Portability — Linux as a future target

Windows-only ships in v1, but the following decisions are made now so a Linux port is a matter of filling in one module rather than unpicking the core:

- **All platform code behind a `PlatformFs` trait** in `platform/`, with `#[cfg(windows)] windows.rs` and a `linux.rs` stub. The engine depends only on the trait. No `windows` crate import anywhere outside `platform/` and `cloud/`.
- **Volume identity is opaque.** The schema stores `(identity_kind, identity)` rather than a Windows-shaped GUID path. Windows populates the volume GUID path; Linux will use the filesystem UUID from `/dev/disk/by-uuid` with the mount point resolved via `/proc/mounts`.
- **Drive-type gate is a trait method.** Windows uses `GetDriveTypeW`; Linux will reject network filesystems by fstype (`nfs`, `cifs`, `sshfs`, `fuse.*`) and accept block-backed mounts.
- **Paths are `Path`/`OsStr` throughout, never `String`.** The `\\?\` long-path prefix is applied only inside `platform/windows.rs`.
- **No assumption about case sensitivity.** NTFS is case-insensitive, ext4 is case-sensitive; the diff logic stores paths as-is and compares using a platform-supplied comparator.
- **Metadata:** mtime is preserved everywhere and is the only cross-platform guarantee. POSIX mode is a Linux-side concern; NTFS ACLs are explicitly not portable and not preserved.
- **OneDrive has no official Linux client**, so `cloud/` is Windows-only and its trait methods are a no-op on other platforms. Nothing in the engine may assume placeholders exist.
- **CI builds and tests on `ubuntu-latest` from M0**, so portability cannot silently rot even while no Linux release is produced.

---

## 3. Repository, build and installation

### 3.1 Layout

```
shelv-app/
├─ src/                      # React + TS frontend
│  ├─ components/            # RuleTable, RuleEditor, TagPicker, RunHistory, StatusPill
│  ├─ lib/                   # typed invoke() wrappers, event subscriptions
│  ├─ types/                 # generated from Rust via ts-rs
│  └─ styles/
├─ src-tauri/
│  ├─ src/
│  │  ├─ main.rs  lib.rs
│  │  ├─ commands/  engine/  cloud/  platform/  volumes/  scheduler/  store/  safety/
│  │  └─ migrations/
│  ├─ capabilities/          # Tauri 2 permission sets (§4.2)
│  ├─ tauri.conf.json
│  └─ Cargo.toml
├─ .github/
│  ├─ workflows/ci.yml  release.yml
│  └─ dependabot.yml
├─ docs/  PLAN.md  ARCHITECTURE.md  SECURITY.md
├─ CONTRIBUTING.md  README.md  LICENSE
└─ package.json  pnpm-lock.yaml
```

Use **`ts-rs`** to generate TypeScript types from the Rust structs. It eliminates the most common Tauri bug class — silently drifting IPC payload shapes — for almost no cost.

### 3.2 CI (`ci.yml`, on push/PR)

- Rust: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo deny check` (advisories, licences, bans).
- Frontend: `pnpm lint`, `pnpm typecheck`, `pnpm test` (Vitest).
- Build matrix: `windows-latest` (the shipping target) **and** `ubuntu-latest` (portability guard, §2.6).
- Integration tests against temp directories covering the cases that actually hurt: long paths, FAT32 mtime granularity, symlink/junction escape, destination-inside-source, simulated placeholder attributes, mid-run cancellation, interrupted copy.
- Actions pinned by commit SHA.

### 3.3 Release (`release.yml`, on tag `v*`)

`tauri-apps/tauri-action` on `windows-latest` produces MSI (WiX) and NSIS `.exe`, signs the updater artifacts with a minisign key from `TAURI_SIGNING_PRIVATE_KEY`, and publishes a **draft** GitHub Release with `latest.json`. Drafting rather than auto-publishing keeps a human in the loop for a tool that holds delete permissions.

### 3.4 Installation

1. **Primary:** download `Shelv_x.y.z_x64-setup.exe` from Releases and run. Per-user install, no admin required.
2. **Auto-update:** the Tauri updater checks `latest.json` on GitHub Releases and verifies a minisign signature against a public key compiled into the binary. Disableable in settings.

**Unsigned in v1, as decided.** An unsigned installer triggers a Microsoft Defender SmartScreen warning ("Windows protected your PC"). The README documents the _More info → Run anyway_ path plainly. Note that this remains sound only while distribution is personal; if Shelv is ever shared publicly, teaching users to click through SmartScreen is a poor default and signing should be revisited. Because the repository is private, GitHub Release assets are not publicly downloadable — updater and download URLs will need authenticated access, or the release process moves to a public repo when the project does.

### 3.5 Conventions

`main` protected, squash merges, Conventional Commits, PRs required. Tags `v0.1.0` onward drive `release.yml`.

---

## 4. Security

### 4.1 Threat model

**Assets:** the user's source files (integrity above all — a backup tool that corrupts or deletes originals is worse than none), the backup copies, the rule database.
**Trust boundaries:** WebView ↔ Rust core; Rust core ↔ filesystem; app ↔ network; supply chain.
**Non-goal:** defending against an attacker already executing code as this user — they do not need Shelv.

The strongest property of this design is architectural and follows from the stated intent: **Shelv holds no credentials.** It reads the _local_ OneDrive folder on disk and relies on the OneDrive client for hydration; it never authenticates to Microsoft Graph. No OAuth, no token store, no refresh flow, nothing to steal. This is a binding design constraint — adding any remote destination destroys it and requires this model to be rewritten.

| #   | Threat                                                                                                               | Mitigation                                                                                                                                                                                                                                                                                                                                                            |
| --- | -------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1  | Crafted filename rendered in the table achieves script execution in the WebView, which then drives the backup engine | Strict CSP (`default-src 'self'`); no `dangerouslySetInnerHTML`; no `eval`; `withGlobalTauri: false`. Decisively, **the frontend holds no filesystem capability** — commands take rule/destination IDs and Rust resolves paths from the database. A compromised WebView can start a backup the user already configured; it cannot invent a path                       |
| T2  | Path traversal or symlink/junction escape copies or overwrites outside the intended tree                             | Canonicalise both ends; reject any resolved path that is not a descendant of the rule root; use `symlink_metadata` and **do not follow links by default**; depth cap; cycle detection                                                                                                                                                                                 |
| T3  | Wrong-drive clobber after a letter reassignment                                                                      | Match on stable volume identity before any write (§1.1a, §2.6). Mismatch ⇒ UNAVAILABLE, never a mount-point fallback                                                                                                                                                                                                                                                  |
| T4  | Destructive mirror deletes user data                                                                                 | Deletions **off by default**, explicit per-rule opt-in; refuse destination inside source and vice versa; refuse bare drive roots, `C:\Windows`, `C:\Program Files`, user profile root; dry run shows the deletion count before the first destructive run; delete to Recycle Bin where available; temp+atomic rename so an interrupted run never truncates a good copy |
| T5  | Cloud hydration fills the system disk, or releases a file the user wanted kept local                                 | Preflight space check on both volumes; bounded hydrate→copy→release batches; per-rule hydration budget; record pre-existing pin state and **only release files Shelv itself hydrated** (§1.2)                                                                                                                                                                         |
| T6  | Hydration reads produce truncated stubs that look like successful backups                                            | Never memory-map a sync root; per-file hydration timeout; verify post-hydration size against the placeholder's logical size before the copy counts as successful; any mismatch or timeout marks the file skipped and the run **Partial**                                                                                                                              |
| T7  | Data exfiltration                                                                                                    | No telemetry, no analytics, no crash reporting. The only outbound request is the updater to GitHub, and it is disableable                                                                                                                                                                                                                                             |
| T8  | Malicious update ⇒ code execution                                                                                    | Updater verifies a minisign signature against a public key compiled into the binary; the private key lives only in Actions secrets; releases are built by tagged CI, never a laptop, and drafted for human confirmation                                                                                                                                               |
| T9  | Supply-chain compromise via a dependency                                                                             | `cargo deny` and `pnpm audit` in CI; Dependabot on cargo, npm and actions; lockfiles committed; actions pinned by SHA; a deliberately small dependency surface                                                                                                                                                                                                        |
| T10 | Privilege escalation                                                                                                 | Runs strictly as the logged-in user. No elevation, no Windows service, no admin installer. This is also _required_ for OneDrive hydration to work at all (§1.2). Accepted consequence: unreadable files are not backed up, and are reported as skipped                                                                                                                |
| T11 | Rule database tampering or disclosure                                                                                | `%LOCALAPPDATA%\Shelv` with default per-user ACLs. Paths and metadata only — no credentials, because none exist. The README states plainly that logs contain file paths                                                                                                                                                                                               |
| T12 | Locked files (an open Lightroom catalog) copied in a torn state                                                      | Open share-read; detect sharing violations; record as skipped and mark the run **Partial**. Never report success on a torn copy                                                                                                                                                                                                                                       |

### 4.2 Enforcing "local disks and connected drives only"

Two layers, both re-checked immediately before the first write of every run — configuration-time-only validation is bypassable by anything that edits the database:

1. **Drive-type gate.** `GetDriveTypeW` must return `DRIVE_FIXED` or `DRIVE_REMOVABLE`. `DRIVE_REMOTE` (SMB/network shares, mapped letters, UNC paths), `DRIVE_CDROM` and `DRIVE_RAMDISK` are rejected at rule creation _and_ at run time, so a rule cannot be repointed at a network share later.
2. **Tauri 2 capabilities.** Grant only the permissions actually used (`dialog:allow-open`, `notification:default`, `updater:default`, autostart, single-instance). The `fs` and `shell` plugins are **not** enabled for the frontend at all. Paths enter the system only through the native folder picker — the OS-level consent step — and are then stored as volume-relative references.

### 4.3 Deliberate v1 exclusions

Archive encryption is out. Legacy ZipCrypto is cryptographically broken and must never be offered; AES-256 zip or `age` would be sound but bring key management and an unrecoverable-data failure mode a v1 does not need. For a destination drive that needs protection, BitLocker To Go is the correct answer and ships with Windows — document that rather than reimplement it.

### 4.4 Private-repository caveat

The repository is private for now and may become public. Two consequences to respect from the first commit: never commit anything that would be harmful on publication (no real paths from the developer's machine in fixtures or test data, no keys, no personal identifiers), and note that the "no telemetry, verify it in the source" claim in T7 is only meaningful to users who can read the source. Until the repo is public, that guarantee rests on trust rather than inspection.

---

## 5. Roadmap

| Phase                      | Deliverable                                                                                                                                                                                     |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **M0 — Skeleton**          | Tauri 2 + React scaffold, SQLite store and migrations, platform abstraction boundary, locked-down capabilities, CI green on Windows and Linux, dark theme shell, rule table rendering stub data |
| **M1 — Core engine**       | Volume enumeration with stable identity; planner (walk, exclude, diff); copier (parallel, temp+rename, long paths); Mirror layout; dry run; run history. Testable without UI                    |
| **M2 — Usable app**        | Rule CRUD, folder picker, availability column, Backup Now, live progress, colour-coded tags, run results                                                                                        |
| **M3 — Automation**        | Scheduler with catch-up and run-on-connect; tray icon; run at login; notifications; Snapshot layout with retention                                                                              |
| **M4 — Cloud + packaging** | OneDrive hydrate / hydrate-and-release with budgets and pin-state restore; Zip with Zip64 and store/deflate; compression-ratio readout; rule import/export                                      |
| **M5 — Release**           | MSI + NSIS via `tauri-action`, signed updater, docs, `v1.0.0`                                                                                                                                   |
| **v1.1**                   | BLAKE3 verification mode; hard-linked snapshots; Volume Shadow Copy for locked files; restore UI                                                                                                |

### 5.1 Engine acceptance criteria for v1.0

Non-negotiable, because these are the cases where a backup tool actively hurts you:

- Interrupting a run — killing the process or yanking the drive — never leaves a corrupted or truncated file at the destination.
- A rule whose destination volume is absent never runs and never silently retargets.
- Deletions never occur without an explicit per-rule opt-in, and are always previewable.
- A run that could not read every file reports **Partial**, never OK.
- Hydration never exceeds the configured budget or available free space, and never releases a file the user had pinned.

# Shelv — Design & Build Plan

> Status: proposal for review. Nothing is implemented yet.
> Scope of this document: feature review (§1), architecture (§2), repo/build/install (§3), security (§4), roadmap and open decisions (§5).

**Naming:** the request says "Shelve", the repository is `shelv-app`. This document uses **Shelv** to match the repo. One-line change if you prefer "Shelve".

---

## 1. Review of the requested features

The seven requirements are coherent and buildable. Below: things in the current spec that will cause real problems (1.1), small additions worth taking (1.2), and things deliberately left out to protect scope (1.3).

### 1.1 Corrections — problems the spec as written will hit

**a) Drive letters are not stable identifiers.**
The mock-up identifies destinations as `D:/...` and `E:/..`. Windows assigns letters from a registry mapping; unplug two drives, plug them back in a different order, and `E:` can be a different physical disk. A rule keyed on a letter can therefore write a backup onto the wrong drive, and "mirror with deletions" onto the wrong drive is data loss.

Fix: store each destination as `{volume GUID path, volume serial, volume label, relative path}` and resolve the current letter at run time. Verify the GUID matches before the first write. If it does not match, the rule is UNAVAILABLE — never silently fall back to the letter. This is cheap to build and removes the single worst failure mode in the design.

**b) "Lossless compression only" is already guaranteed — the real risks are different.**
Every general-purpose archive codec (Deflate/zip, zstd, LZMA, bzip2) is lossless by construction; lossy compression only exists in media-specific codecs, which an archiver never applies. So requirement 4 is satisfied by any standard `.zip`. Two things actually do need handling:

- **Zip64.** The original ZIP format caps entries at 65,535 and both archive and member size at 4 GiB. A Lightroom export folder will exceed this. Zip64 must be enabled (it is opt-in in some libraries).
- **Compressing already-compressed data is near-pointless.** RAW, JPEG, PNG, MP4 and similar are already entropy-coded; Deflate typically recovers a low single-digit percentage at best while costing full-throughput CPU and, more importantly, turning a resumable incremental file copy into an all-or-nothing archive rewrite. Recommendation: offer per-rule `Compression: None | Store | Deflate`, default **None** for image rules and **Deflate** for document rules, and show the achieved ratio after a run so the choice is evidence-based rather than a guess.

**c) A timestamped-subfolder rule grows without bound.**
"Monthly, timestamped" on a Lightroom catalog fills a drive and then starts failing. Retention is not an optional extra for this backup style; it is part of it. Minimum viable: per-rule `keep last N snapshots` and/or `keep snapshots newer than X days`, with pruning after a *successful* run only.

**d) Frequency alone is the wrong trigger for a removable-drive rule.**
Four of the six rules in the mock-up target an unavailable drive. A "monthly on the 1st" rule whose drive is connected on the 3rd simply never runs. Two behaviours are needed and are small:

- **Catch-up:** if a scheduled run was missed (machine off, drive absent), run it at the next opportunity rather than skipping to the next period.
- **Run on connect:** when a volume a rule depends on appears, queue that rule. For the described workflow — external drives connected occasionally — this is the primary trigger, and frequency is really "don't run more often than".

**e) The table has no run *result* column.**
"Last Backup: 14-Sept-26" does not say whether that backup worked. A backup tool that cannot distinguish "ran" from "succeeded" is actively misleading. Add a result state per rule: `OK / Partial (n files skipped) / Failed / Never run`, and keep a per-run log.

**f) OneDrive Files On-Demand needs explicit handling.**
Requirement 7 includes the local OneDrive folder. With Files On-Demand, most files there are *placeholders*: directory entries carrying `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `FILE_ATTRIBUTE_OFFLINE` with no local content. Reading one triggers a download. Two distinct failure modes follow:

- A naive recursive copy **hydrates the entire cloud library**, downloading everything and filling the local disk.
- A tool that reads the stub without hydrating backs up **0-byte or truncated files** that look like successful copies.

Fix: detect the attributes before opening, and make the policy explicit per rule — `Skip placeholders (default) | Hydrate and back up`. Report the skipped count in the run summary so "my backup is smaller than my folder" is never a mystery. Treat OneDrive paths as **read-only sources**; Shelv should never write into a sync root.

**g) Filesystem differences between source and destination.**
External drives are frequently exFAT or FAT32. Consequences worth handling up front, all cheap:
- FAT32 stores mtime at 2-second granularity (exFAT 10 ms). An incremental comparison using exact mtime equality will re-copy every file, every run. Use a 2-second tolerance.
- NTFS ACLs, alternate data streams and long paths do not survive to exFAT. Paths over 260 characters need the `\\?\` prefix on Windows, or the copy fails on deeply nested folders — common in Lightroom trees.

**h) Overwrite mode must not mean "delete the destination first".**
If "direct overwrite" is implemented as wipe-then-copy, an interrupted run leaves no backup at all. It must be an in-place incremental sync: copy changed files to a temp name, `rename` into place (atomic on the same volume), and only remove extraneous files if the user has explicitly enabled deletions.

### 1.2 Additions worth taking (small, high value)

| Addition | Why | Cost |
|---|---|---|
| **Dry run / preview** | Shows files to copy, bytes, and *deletions* before anything is touched. The single best guard against a mis-configured rule. | Low — it is the copy planner without the execute step |
| **Exclude patterns (globs)** | Skip `Thumbs.db`, `*.tmp`, `.lrcat-journal`, `.lrprev` previews. Cuts a Lightroom backup substantially. | Low |
| **Preflight free-space check** | Refuse to start a run that cannot fit, rather than filling the drive and failing halfway. | Low |
| **Per-run history + log** | Powers the result column in 1.1(e); needed for any diagnosis. | Low |
| **Verification mode** | Post-copy comparison: size+mtime (fast, default) or BLAKE3 hash (opt-in, for archival runs). | Low–medium |
| **Tray + desktop notification** | Requirement for background operation; a background backup that fails silently is worse than no backup. | Low |
| **Import/export rules as JSON** | Config backup and machine migration. | Low |

### 1.3 Explicitly out of scope for v1

Listed so they are declined deliberately, not forgotten: cloud/remote destinations of any kind (S3, network shares, OneDrive *API*), encrypted archives, block-level or file-delta incremental (rsync-style), Volume Shadow Copy for locked files, restore-from-backup UI, multi-user/server mode, mobile. Hard-linked snapshots (Time Machine-style space-efficient timestamped folders) are attractive but deferred to v1.1 — see §5.

Note on locked files: Lightroom holds its catalog open. Without VSS, Shelv cannot copy a locked file. v1 behaviour is to detect the sharing violation, report the file as skipped, and mark the run **Partial** — never to claim success. VSS is the v1.1 answer.

### 1.4 Revised feature model

The requested "styles" decompose more cleanly into three orthogonal axes, which removes combinatorial special cases from the code and the UI:

```
layout      : Mirror | Snapshot(timestamped subfolder)
packaging   : Files  | Zip (store | deflate)
retention   : n/a for Mirror; KeepLastN / KeepDays for Snapshot
deletions   : Off (default) | On          -- Mirror only
```

`Backup Type: Overwrite` in the mock-up = `Mirror + Files + deletions off`.
`Backup Type: Subfolder` = `Snapshot + Files + KeepLastN`.

---

## 2. Language and architecture

### 2.1 Stack recommendation

**Tauri 2 — Rust core, React + TypeScript frontend.**

| | **Tauri 2 (Rust + TS)** | .NET 9 + Avalonia/WinUI | Electron + TS |
|---|---|---|---|
| Installer size | ~10 MB | ~70 MB self-contained | ~100–150 MB |
| Idle RAM | low | low | high |
| Bulk file I/O | excellent, easy parallelism | very good | weak (Node I/O) |
| Deep Win32 (VSS, cloud filter, device notify) | available via `windows` crate, more code | first-class | poor |
| Frontend privilege isolation | capability system, enforced | n/a | manual, easy to get wrong |
| Table-heavy UI dev speed | high | medium | high |
| Cross-platform later | yes | Avalonia only | yes |

Rationale: the workload is "walk millions of directory entries, hash and copy bytes in parallel, with a rich table UI on top". Rust handles the first half with `rayon`/`jwalk` and no GC pauses; a web frontend handles the second half far faster than XAML. Tauri's capability model (§4) is also a direct, enforced answer to requirement 4's security intent rather than a convention.

**Runner-up:** .NET 9 + Avalonia, if C# is preferred or if Volume Shadow Copy support is wanted early — VSS is materially easier from .NET. This is a real trade; it is the one stack decision worth confirming before code is written (§5.3).

Tauri 2 is mature (2.10.x current line, active releases through 2026) and bundles MSI and NSIS installers natively.

### 2.2 Component architecture

The design rule is: **the WebView never touches the filesystem.** All I/O lives in Rust, behind typed commands that take rule IDs, not paths.

```
┌──────────────────────── WebView (React 19 + TS) ─────────────────────────┐
│  RuleTable (TanStack Table) · RuleEditor · TagManager · RunHistory       │
│  Zustand store  ·  no fs access  ·  strict CSP  ·  no eval               │
└───────────────┬──────────────────────────────▲──────────────────────────┘
      invoke()  │ typed commands (rule ids)    │ events: progress, volume,
                ▼                              │         run-complete
┌──────────────────────── Rust core (tokio) ───────────────────────────────┐
│  commands/     IPC surface; validates + authorises every call            │
│  engine/       plan → execute → verify → prune                           │
│      planner     walk, filter, diff (size + mtime ±2s), build work list  │
│      copier      parallel copy, temp+rename, long-path, retry            │
│      archiver    zip (zip64), store|deflate                              │
│      verifier    size/mtime or BLAKE3                                    │
│      retention   prune snapshots after successful run only               │
│  volumes/      enumerate volumes, stable IDs, drive-type gate,           │
│                OneDrive placeholder detection, arrival watcher           │
│  scheduler/    cron eval, catch-up, run-on-connect, run queue + locks    │
│  store/        SQLite (rusqlite) — rules, tags, volumes, runs, events    │
│  safety/       path canonicalisation, allowlist, destructive-op guards   │
└───────────────┬──────────────────────────────────────────────────────────┘
                ▼
         filesystem  ·  local disks, external volumes, local OneDrive folder
```

**Concurrency:** one `tokio` runtime. The scheduler owns a run queue; at most one run per destination volume at a time (a per-volume mutex) so two rules cannot interleave writes on one disk. Within a run, directory walking and file copying are parallelised with a bounded worker pool — sized ~4 for spinning disks and higher for SSDs, configurable, because oversubscribing a mechanical external drive makes it slower, not faster. Progress is streamed to the UI as throttled events (≤10/s) so a million-file run does not flood the IPC channel.

**Cancellation:** every run carries a `CancellationToken`; the copier checks it between files. A cancelled run leaves no partial file behind because of the temp+rename discipline.

### 2.3 Data model (SQLite via `rusqlite`, bundled)

```sql
CREATE TABLE volume (
  id             INTEGER PRIMARY KEY,
  guid_path      TEXT NOT NULL UNIQUE,   -- \\?\Volume{GUID}\  — stable identity
  serial         TEXT,                   -- from GetVolumeInformation
  label          TEXT,
  filesystem     TEXT,                   -- NTFS | exFAT | FAT32 …
  drive_type     TEXT NOT NULL,          -- fixed | removable   (remote/cdrom rejected)
  last_seen_at   INTEGER,
  last_letter    TEXT                    -- display only, never used for resolution
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
  placeholders    TEXT NOT NULL DEFAULT 'skip',   -- skip | hydrate
  follow_symlinks INTEGER NOT NULL DEFAULT 0,
  verify          TEXT NOT NULL DEFAULT 'size_mtime', -- size_mtime | blake3
  excludes        TEXT,                  -- JSON array of globs
  created_at      INTEGER NOT NULL
);

-- a rule may have several destinations ("add backup directories", plural)
CREATE TABLE destination (
  id            INTEGER PRIMARY KEY,
  rule_id       INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
  volume_id     INTEGER NOT NULL REFERENCES volume(id),
  dest_rel      TEXT NOT NULL,
  sort_order    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE tag (
  id     INTEGER PRIMARY KEY,
  name   TEXT NOT NULL UNIQUE,
  colour TEXT NOT NULL              -- pastel hex from a curated, contrast-checked set
);
CREATE TABLE rule_tag (
  rule_id INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
  tag_id  INTEGER NOT NULL REFERENCES tag(id)  ON DELETE CASCADE,
  PRIMARY KEY (rule_id, tag_id)
);

CREATE TABLE run (
  id              INTEGER PRIMARY KEY,
  rule_id         INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
  destination_id  INTEGER NOT NULL REFERENCES destination(id) ON DELETE CASCADE,
  trigger         TEXT NOT NULL,   -- manual | schedule | catch_up | on_connect
  started_at      INTEGER NOT NULL,
  finished_at     INTEGER,
  result          TEXT,            -- ok | partial | failed | cancelled
  files_copied    INTEGER DEFAULT 0,
  files_skipped   INTEGER DEFAULT 0,
  files_deleted   INTEGER DEFAULT 0,
  bytes_copied    INTEGER DEFAULT 0,
  placeholders_skipped INTEGER DEFAULT 0,
  compressed_bytes     INTEGER,    -- powers the "was compression worth it" readout
  error           TEXT,
  snapshot_path   TEXT
);

CREATE TABLE run_event (   -- per-file problems; capped and rotated
  id       INTEGER PRIMARY KEY,
  run_id   INTEGER NOT NULL REFERENCES run(id) ON DELETE CASCADE,
  level    TEXT NOT NULL,  -- warn | error
  path     TEXT NOT NULL,
  message  TEXT NOT NULL
);
```

Schema migrations via `refinery` or hand-rolled `user_version` steps — either is fine at this size.

### 2.4 Key Rust dependencies

`tauri` 2 · `rusqlite` (bundled SQLite) · `tokio` · `serde`/`serde_json` · `jwalk` or `walkdir` (parallel directory traversal) · `rayon` · `zip` (Zip64 enabled) · `blake3` (SIMD-accelerated, `rayon` feature for multithreaded hashing) · `globset` (excludes) · `croner` (schedule evaluation) · `chrono` · `windows` (Win32: `GetVolumeInformationW`, `GetDriveTypeW`, `FindFirstVolumeW`, file attributes) · `tracing` + `tracing-subscriber` · `thiserror`/`anyhow` · `sysinfo` (free space).

Tauri plugins: `autostart` (run at login), `notification`, `updater`, `single-instance`, `dialog` (native folder picker), `log`, `window-state`.

### 2.5 Frontend

React 19 + TypeScript + Vite. **TanStack Table v8** for the rule table (sorting, per-tag filtering, column sizing — exactly the mock-up's shape). Tailwind v4 for styling. Zustand for client state; Tauri events push server state. Virtualised rows only if the rule count ever justifies it — it will not.

**Theme:** dark neutral base with pastel accents, as requested. Define the palette as CSS custom properties on `:root` so tags and status colours come from one place. Two hard constraints: status must never be conveyed by colour alone (AVAILABLE/UNAVAILABLE gets an icon and text, not just green/red), and pastel-on-dark must still clear WCAG AA (4.5:1) for text — pastels fail this easily, so the tag palette should be contrast-checked once and fixed, not picked ad hoc per tag.

**Table columns** (mock-up plus §1.1 corrections): Tags · Name · Source · Destination · Availability (with volume label) · Layout · Compression · Schedule · **Last Result** · Last Run · Next Run · Actions.

---

## 3. Repository, build and installation

### 3.1 Layout

```
shelv-app/
├─ src/                      # React + TS frontend
│  ├─ components/            # RuleTable, RuleEditor, TagPicker, RunHistory, StatusPill
│  ├─ lib/                   # typed invoke() wrappers, event subscriptions
│  ├─ types/                 # generated from Rust (ts-rs) — single source of truth
│  └─ styles/
├─ src-tauri/
│  ├─ src/
│  │  ├─ main.rs  lib.rs
│  │  ├─ commands/   engine/   volumes/   scheduler/   store/   safety/
│  │  └─ migrations/
│  ├─ capabilities/          # Tauri 2 permission sets (see §4.2)
│  ├─ tauri.conf.json
│  └─ Cargo.toml
├─ .github/
│  ├─ workflows/ci.yml  release.yml
│  └─ dependabot.yml
├─ docs/  PLAN.md  SECURITY.md  ARCHITECTURE.md
├─ CONTRIBUTING.md  README.md  LICENSE
└─ package.json  pnpm-lock.yaml
```

Use **`ts-rs`** to generate TypeScript types from the Rust structs at build time. It removes the most common bug class in a Tauri app — silently drifting IPC payload shapes — for almost no cost.

### 3.2 CI (`ci.yml`, on push/PR)

- Rust: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo deny check` (advisories, licences, banned crates).
- Frontend: `pnpm lint`, `pnpm typecheck`, `pnpm test` (Vitest).
- Build job on `windows-latest` to catch platform-specific breakage every PR.
- Integration tests run the engine against temp directories with fixtures for the nasty cases: long paths, FAT32 mtime granularity, symlink/junction escape attempts, destination-inside-source, simulated placeholder attributes, mid-run cancellation.
- Pin actions by commit SHA.

### 3.3 Release (`release.yml`, on tag `v*`)

`tauri-apps/tauri-action` on `windows-latest` produces both an MSI (WiX) and an NSIS `.exe`, signs the updater artifacts with a minisign key held in `TAURI_SIGNING_PRIVATE_KEY`, and publishes a **draft** GitHub Release with `latest.json` attached. Drafting rather than auto-publishing means a human confirms each release — worth it for a tool with delete permissions.

### 3.4 Installation paths for users

1. **Primary:** download `Shelv_x.y.z_x64-setup.exe` from GitHub Releases and run it. Per-user install, no admin required.
2. **Auto-update:** Tauri updater checks `latest.json` on GitHub Releases, verifies the minisign signature against a public key compiled into the binary, and prompts. Disableable in settings.
3. **Later:** a winget manifest (`eltam-nz.Shelv`) so `winget install Shelv` works. Requires a signed installer to be accepted — see below.

**Code signing — be realistic about this.** An unsigned installer triggers a Microsoft Defender SmartScreen warning ("Windows protected your PC") that most users read as malware. Options:

| Option | Cost | Effect |
|---|---|---|
| Ship unsigned | £0 | SmartScreen warning on every release; documented workaround in the README |
| Azure Artifact Signing (formerly Trusted Signing) | ~$10/month | No instant trust, but publisher reputation accrues across consistently signed releases; individual/self-employed sign-up is open, currently limited to US/Canada for individuals |
| EV certificate | ~$300–500/year | Strongest immediate reputation |

For a personal tool, ship unsigned in v1 and document it. Revisit if it is ever distributed more widely. Azure Artifact Signing's geographic restriction on individual accounts may rule it out from NZ — worth checking before budgeting for it.

### 3.5 Branch and commit conventions

`main` protected, squash merges, Conventional Commits, PRs required. Tag `v0.1.0` onward; `release.yml` fires on tags only.

---

## 4. Security

### 4.1 Threat model

**Assets:** the user's source files (integrity above all — a backup tool that corrupts or deletes originals is worse than no backup tool), the backup copies, and the rule database.
**Trust boundaries:** (1) WebView ↔ Rust core, (2) Rust core ↔ filesystem, (3) app ↔ network, (4) supply chain.
**Non-goal:** defending against an attacker who already has code execution as this user. Such an attacker does not need Shelv.

The strongest security property of this design is architectural and comes free from the stated intent: **Shelv holds no credentials.** It touches the *local* OneDrive folder on disk, not the Microsoft Graph API. There is no OAuth, no token store, no refresh flow, nothing to steal. That should be a stated design constraint, not an accident — if remote destinations are ever added, this property is lost and the threat model must be rewritten.

| # | Threat | Mitigation |
|---|---|---|
| T1 | Malicious content (a crafted filename) rendered in the table leads to script execution in the WebView, which then drives the backup engine | Strict CSP (`default-src 'self'`); no `dangerouslySetInnerHTML`; no `eval`; `withGlobalTauri: false`. Critically, **the frontend has no filesystem capability at all** — commands accept rule/destination IDs, and the Rust side resolves paths from the database. A compromised WebView can start a backup the user already configured; it cannot invent a path |
| T2 | Path traversal or symlink/junction escape copies or overwrites files outside the intended tree | Canonicalise both ends; reject any resolved path that is not a descendant of the rule's root; use `symlink_metadata` and **do not follow links by default**; depth cap; cycle detection |
| T3 | Wrong-drive clobber after a letter reassignment | Match on volume GUID path + serial before any write (§1.1a). Mismatch ⇒ UNAVAILABLE, never a letter fallback |
| T4 | Destructive mirror deletes user data | Deletions **off by default** and require explicit per-rule opt-in; refuse destination inside source and vice versa; refuse bare drive roots, `C:\Windows`, `C:\Program Files`, and user profile root; dry-run shows the deletion count before the first destructive run; delete to Recycle Bin where available; temp-file + atomic rename so an interrupted run never truncates a good copy |
| T5 | OneDrive placeholders: mass unintended hydration, or silently backing up empty stubs | Detect `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `FILE_ATTRIBUTE_OFFLINE` before opening; skip by default, hydrate only on explicit opt-in; report counts. Never write into a sync root |
| T6 | Data exfiltration | No telemetry, no analytics, no crash reporting. The **only** outbound request is the updater to GitHub, and it is disableable. This is easy to verify in-source and should be documented so users can |
| T7 | Malicious update ⇒ code execution | Tauri updater verifies a minisign signature against a public key compiled into the binary; private key lives only in GitHub Actions secrets; releases are built by tagged CI, never from a laptop; releases are drafted for human confirmation |
| T8 | Supply-chain compromise via a dependency | `cargo deny` (advisories + licences + bans) and `pnpm audit` in CI; Dependabot on cargo, npm and actions; lockfiles committed; actions pinned by SHA; keep the dependency surface deliberately small |
| T9 | Privilege escalation | Runs strictly as the logged-in user. No elevation, no Windows service, no admin installer. Consequence, accepted: files the user cannot read are not backed up, and are reported as skipped |
| T10 | Rule database tampering or disclosure | Stored under `%LOCALAPPDATA%\Shelv` with default per-user ACLs. Contains paths and metadata only — no credentials, because none exist. Note honestly in the README that logs contain file paths |
| T11 | Locked files (e.g. an open Lightroom catalog) copied in a torn, unusable state | Open with share-read; detect sharing violations; record the file as skipped and mark the run **Partial**. Never report success on a torn copy. VSS deferred (§1.3) |

### 4.2 Enforcing "local disks and connected drives only"

Requirement 4's intent — keep this to local OneDrive and physically connected disks — is enforced at two layers:

1. **Drive-type gate.** `GetDriveTypeW` must return `DRIVE_FIXED` or `DRIVE_REMOVABLE`. `DRIVE_REMOTE` (SMB/network shares, including mapped letters and UNC paths), `DRIVE_CDROM` and `DRIVE_RAMDISK` are rejected at rule-creation time and re-checked at run time. A rule cannot be pointed at a network share even by editing it later.
2. **Tauri 2 capabilities.** Grant only the plugin permissions actually used (`dialog:allow-open`, `notification:default`, `updater:default`, autostart, single-instance). Do **not** enable the `fs` or `shell` plugins for the frontend at all. Paths enter the system only through the native folder picker, which is the OS-level consent step, and are then stored server-side as volume-relative references.

Both layers are checked again immediately before the first write of every run, not just at configuration time — configuration-time-only validation is bypassable by anything that edits the database.

### 4.3 Deliberate v1 security exclusions

Archive encryption is out. Legacy ZipCrypto is cryptographically broken and should never be offered; AES-256 zip or `age` would be sound but drags in key management and an unrecoverable-data failure mode that a v1 does not need. If the destination drive needs protection, BitLocker To Go is the correct answer and is already built into Windows. Document that recommendation rather than reimplementing it.

---

## 5. Roadmap and open decisions

### 5.1 Milestones

| Phase | Deliverable |
|---|---|
| **M0 — Skeleton** | Tauri 2 + React scaffold, SQLite store and migrations, CI green on Windows, dark theme shell, empty rule table |
| **M1 — Core engine** | Volume enumeration with stable IDs; planner (walk, exclude, diff); copier (parallel, temp+rename, long-path); Mirror layout; dry run; run history. CLI-testable without UI |
| **M2 — Usable app** | Rule CRUD, folder picker, destination availability column, Backup Now, live progress, colour-coded tags, run results |
| **M3 — Automation** | Scheduler with catch-up and run-on-connect; tray icon; run at login; notifications; Snapshot layout with retention |
| **M4 — Packaging** | Zip with Zip64 and store/deflate choice; BLAKE3 verification; compression-ratio readout; rule import/export |
| **M5 — Release** | MSI + NSIS via `tauri-action`, signed updater, docs, `v1.0.0` |
| **v1.1** | Hard-linked snapshots (large space saving for timestamped backups); Volume Shadow Copy for locked files; restore UI |

### 5.2 What "done" means for the engine

Non-negotiable acceptance criteria before v1.0, because they are the cases where a backup tool actually hurts you:

- Interrupting a run (kill the process, yank the drive) never leaves a corrupted or truncated file at the destination.
- A rule whose destination volume is absent is never run, and never silently retargets.
- Deletions never occur without an explicit per-rule opt-in, and are always previewable.
- A run that could not read every file reports **Partial**, never OK.
- Placeholder skip counts are surfaced, so a smaller backup than source is always explained.

### 5.3 Decisions to confirm before implementation

1. **Name:** Shelv (matches repo) or Shelve?
2. **Stack:** Tauri 2 + Rust as recommended, or .NET 9 + Avalonia if C# is preferred / VSS is wanted early? This is the only choice that is expensive to reverse later.
3. **Platforms:** Windows-only for v1 (the mock-up and OneDrive integration are Windows-shaped), or keep macOS/Linux buildable in CI from the start? Keeping the door open costs little with Tauri but does constrain how much Win32 goes into the core.
4. **Licence:** MIT, Apache-2.0, or private?
5. **Repository visibility:** public or private? (Affects the security-disclosure process and whether the "verifiable no-telemetry" claim in T6 means anything.)

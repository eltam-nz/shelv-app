# Architecture

How Shelv is put together and why. `docs/PLAN.md` is the original proposal and
records the reasoning behind the feature set; this file describes the code as
it actually stands and is kept current.

## Shape

```
shelv-app/
├─ crates/shelv-core/   Rust. Engine, store, platform layer. No GUI dependency.
├─ src-tauri/           Rust. The window and the IPC surface. Thin.
├─ src/                 React + TypeScript. The UI.
└─ scripts/             Checks that CI runs.
```

The split is load-bearing. `shelv-core` has no Tauri dependency, so the engine
can be exercised by tests, by the `volumes` example, and later by a CLI or a
Linux build, without a window. It also means the heavy work compiles and runs
on any platform even while the shipping target is Windows only.

## The two rules that keep it honest

**1. The frontend has no filesystem access.**

Every IPC command takes rule, destination or tag **ids**. Paths are resolved
inside `shelv-core` from the database. A compromised `WebView` can therefore
start the backups the user already configured, but cannot invent a target.
Paths enter the system only through the native folder picker, which is the
operating system's own consent step.

This is enforced, not just intended — see [SECURITY.md](SECURITY.md) and
`src-tauri/src/security_tests.rs`.

**2. OS-specific code lives only in `platform/`.**

The engine depends on the `PlatformFs` and `CloudPlaceholders` traits, never on
a concrete implementation. `scripts/check-platform-boundary.sh` fails CI if the
`windows` crate, `unsafe`, or `allow(unsafe_code)` appears anywhere else.

## Modules

| Module | Responsibility |
|---|---|
| `platform/` | Volume enumeration, stable volume identity, drive-type classification, long-path handling, cloud placeholder attributes. The only place with `unsafe` or OS APIs. |
| `store/` | SQLite. Rules, destinations, tags, known volumes, run history. Schema migrations keyed on `user_version`. |
| `model/` | The domain types, and how each is spelled in the database. |
| `view/` | Aggregates for the UI: a rule plus its tags, destinations, availability and last result. |
| `engine/` | Plan, execute, prune. `engine::planner` decides and writes nothing; `engine::copier` writes. Deletion, snapshots and pruning are **M1.3–M1.4, M3**. |
| `cloud/` | OneDrive hydration, budgets, pin-state restore. **M4.** |
| `scheduler/` | Cron evaluation, catch-up, run-on-connect, the run queue. **M3.** |
| `safety/` | Path canonicalisation and the destructive-operation guards. **M1.** |
| `volumes/` | Volume tracking and identity verification. **M1.** |
| `watch/` | Notices that the set of attached drives has changed, and reports it only when it really has. |

### Where Shelv keeps things

Everything lives under one product-named folder in the user's local
application data — `%LOCALAPPDATA%\Shelv` on Windows:

```
Shelv\
    shelv.db          rules, destinations, tags, drives, run history
    shelv.db-wal      SQLite's write-ahead log
    shelv.db-shm
    EBWebView\        the webview's profile: the window's own preferences
```

Neither path involves the executable, which is why replacing the binary
keeps every rule. That is the point: an update must not lose someone's
backup configuration.

The webview's profile takes a deliberate detour to get there. Tauri files it
under the reverse-DNS identifier by default — `%LOCALAPPDATA%\nz.eltam.shelv`
— which would leave Shelv's data in two differently named sibling folders,
so anyone clearing it out finds one and leaves the other. A `dataDirectory`
in `tauri.conf.json` cannot fix it either: that value is resolved relative to
`<local data>/<window label>`, and an absolute one is discarded. So the
window is built in `src-tauri/src/lib.rs` rather than declared in the config,
purely so `data_directory` can point at the folder the database was opened
from.

That has one consequence worth knowing: the window's **label is
load-bearing**. `capabilities/default.json` scopes its permissions to
`windows: ["main"]`, so a window under any other label starts with none of
them. The cost is `core:default`, and within it `core:event`, which is what
`listen` needs — so the rule table and drives pane would stop following
attached drives, with no error to say why. (Not the folder picker: that runs
`dialog()` from Rust inside `pick_folder`, and the ACL gates commands
invoked from the webview rather than plugin calls the backend makes itself.)
Nothing fails loudly if the label drifts, so `MAIN_WINDOW_LABEL` is a
constant and `security_tests` checks it against the manifest.

The About panel reports both paths, through a `data_locations` command that
reads them from the same directories the app actually opened rather than
rebuilding them from the product name.

### Naming a drive

A drive is shown by the first of these that exists: the **nickname** the user
set, the **filesystem label** the OS reports, `Unnamed drive <serial>`, the
current mount point, the last mount point. The serial comes before the mount
point because it identifies the drive and the mount point does not — two
unlabelled drives that have taken turns in the same port would otherwise both
be called `E:\`, which does not merely fail to help, it says they are the
same drive.

The nickname is stored against the volume identity and is **display only**.
`upsert_volume` and `refresh_volume` are both driven by what the OS reports
and neither touches the column, so the once-a-second refresh cannot erase a
name the user chose.

A destination is rendered drive-first — `Archive 4TB (E:)` over
`Backups\Lightroom` — because that is what is stored. The drive letter is not
part of a destination at all, and is shown only while the drive is attached.

### The drives pane

A resizable split below the rules table, toggled from the top bar, listing
every **recorded** drive with its nickname, Explorer label, current letter,
availability and how many rules use it. Attached-but-unrecorded drives are
deliberately not enumerated: **Add drive…** opens the same native picker
every other location goes through, which is the OS's own consent step, so
there is only ever one way a drive enters Shelv. Forgetting a drive is
refused while any rule points at it.

### Planning a run

`engine::planner::plan` walks the source, applies the rule's ignore patterns,
indexes the destination and produces a `Plan` — copies, deletions,
directories, and every entry it passed over with the reason why. It opens
files for metadata only and writes nothing, so a dry run is literally the
same code the real run will use rather than a second implementation that can
disagree with it.

Four things it has to get right, each of which loses or corrupts data if it
does not:

- **Containment.** Links are never followed by the walk itself. Each one is
  resolved individually and both ends canonicalised; a target outside the
  source root is refused even when the rule follows links, because otherwise
  a link placed inside the source is a way to make Shelv copy anything on the
  machine.
- **mtime tolerance.** Taken from the *destination* volume. FAT32 records
  timestamps to two seconds, so an exact comparison against an NTFS source
  marks every file changed and re-copies the whole tree on every run.
- **Case folding.** Also the destination's, through `CaseSensitivity::fold`.
  It is the destination that decides whether two source names collide, and a
  collision left unfolded would leave mirror treating one of the pair as
  extraneous.
- **Unreadable entries.** Recorded as skips, not swallowed, so the run
  reports `Partial` rather than `Ok`.

`engine::plan_rule` is the id-taking layer above it: it resolves the rule's
source and destinations from the database, plans each destination on that
destination's own terms, and reports an absent drive as `Unavailable` rather
than failing the whole preview. `plan_run` exposes it over IPC.

### Writing the plan

`engine::copier::copy_plan` takes a plan and two roots and creates files.
It never removes one — mirror's deletions are a separate step, kept apart so
the code that can destroy a file is reviewed on its own.

Every file is written to a temporary name **beside its destination** and
renamed into place once its bytes are on disk and `sync_all` has returned.
That is the whole of the first acceptance criterion: a rename within a volume
is atomic, so at no instant does the destination hold a half-written file
under its real name. Pull the drive mid-copy and what is there is either the
previous complete file or the new one. The temp file sits beside the target
rather than in a system temp directory for the same reason — a rename across
volumes is a copy, and would defeat both the atomicity and the point.

Two details that look incidental and are not:

- **The source's mtime is restored on the copy.** The planner diffs on size
  and mtime, so a file that landed stamped with the time of the copy would
  look newer than its source forever and be re-copied on every run. A test
  asserts the second plan of a freshly copied tree is empty.
- **A file that fails is recorded, not fatal.** `CopyReport::failures` names
  each one and the run continues; a backup that stops at the first locked
  file backs up almost nothing. Any failure means the run is `Partial`.

Cancellation is checked between chunks as well as between files, so stopping
during a large file takes effect at once. What was being written is a temp
file, so abandoning it leaves the destination untouched and nothing behind —
both of which are tested, and the atomicity test is verified to fail if
temp-and-rename is removed.

Parallelism is four workers by default and deliberately low: backup
destinations are overwhelmingly external disks, and oversubscribing a
mechanical one turns a sequential write into a seek storm. Progress travels
back through the `CopyObserver` trait rather than a channel, because
`shelv-core` owns no runtime; the shell implements it and throttles the
calls into events.

### Removing what the source no longer has

`engine::trash` is the only code in Shelv that takes a file away from
someone, which is why it is a module of its own rather than a branch inside
the copier.

Nothing in it unlinks. A mirror deletion is a **move** into
`<destination>/.shelv-trash/<timestamp>/`, keeping the file's path within the
backup, so undoing a mistake is a matter of moving a folder back. The trash
folder is on the destination volume, so the move is a rename: it costs
nothing and it cannot half-succeed. There is no copy-then-delete fallback —
if the rename fails, the file stays where it is and the run says so.

This departs from `docs/PLAN.md` §4 T4, which asked for the Recycle Bin.
Windows commonly has the per-volume recycle bin disabled on removable
drives, and `IFileOperation` then deletes permanently, which is the precise
outcome the recycle bin was chosen to prevent; backup destinations are
overwhelmingly removable drives. `PlatformFs::trash` remains on the trait and
still refuses on both platforms, so nothing can reach the permanent path by
accident.

**The trash folder is excluded from the planner's destination index.** Left
in, every previously deleted file would look extraneous, so the next run
would move the trash into the trash and the run after that would do it
again. The test for this is verified to fail when the exclusion is removed.

One folder per run, named in UTC by `engine::stamp` — `2025-09-22T073412Z`.
Local time would read more naturally and would mean carrying a timezone
database and a rule for the hour that happens twice each autumn; two folders
from one evening that sort into the wrong order, or collide, is the worse
failure. The `Z` is there so nobody has to guess. Nothing prunes the trash
yet: retention is M3, and a trash folder that quietly emptied itself before
then would be the same mistake as deleting outright, only later.

## Volume identity

The single most important decision in the data model, because getting it wrong
loses data rather than merely annoying someone.

Windows assigns drive letters from a registry mapping. Unplug two drives and
reconnect them in a different order and `E:` can be a different physical disk.
A rule keyed on a letter can therefore write to the wrong drive, and a
mirror-with-deletions rule onto the wrong drive is destruction.

So a volume is stored as an opaque `(identity_kind, identity)` pair and matched
on identity alone. The mount point is recorded for display and is never used to
resolve anything. Windows uses a volume GUID path; Linux will use the
filesystem UUID from `/dev/disk/by-uuid`.

`view::Availability` has five states rather than the two a mock-up would
suggest, because they need different words and different actions:

| State | Meaning |
|---|---|
| `Available` | Attached, permitted, writable. |
| `Disconnected` | Not attached. Plugging the drive in fixes it. |
| `Refused` | Attached, but a network share, optical media or unclassifiable. Plugging in does not help. |
| `IdentityMismatch` | Something *is* mounted where this destination used to be, but it is a different volume. Never written to. |
| `Unverifiable` | Attached and of a permitted type, but the system reports nothing that identifies it across reconnections. Cannot be told apart from a different drive in the same place, so never written to. |

Availability is re-evaluated once a second by a background thread in the Tauri
shell, which emits `shelv://volumes-changed` to the window only when the
result differs from the previous poll — so the table follows a drive being
plugged in within a second without re-rendering the rest of the time.

`WM_DEVICECHANGE` is the better primitive and is not used: it is Windows-only,
needs a subclassed window procedure and therefore `unsafe` outside the
platform boundary, and cannot be exercised on any other host. Swapping the
timer for it later means calling `VolumeWatch::poll` from that event instead
of from a tick; nothing else moves.

Each poll also re-reads the live label, filesystem and drive type of every
attached volume and overlays them on what is stored, so a drive renamed in
Explorer shows its new name immediately. The write-back is update-only: a
volume enters the database by being picked, never by being attached.

`IdentityMismatch` is the dangerous case. Collapsing it into "unavailable"
would read as "unplugged" and hide the situation that can destroy data, so it
is surfaced as "Different drive".

`Unverifiable` exists because "we could not identify this drive" must not be
allowed to degrade into "assume it is the right one". On Windows the identity
comes from `GetVolumeNameForVolumeMountPointW`; on Linux from
`/dev/disk/by-uuid`. Where neither yields anything — an empty removable bay, a
container without `/dev/disk`, a filesystem with no UUID — the volume is
marked `VolumeIdentityKind::Unverified` and `VolumeInfo::is_usable` returns
false. Nothing that decides whether a backup may be written tests for a
specific identity variant; everything goes through
`VolumeIdentityKind::is_stable`, so a future identity scheme is refused until
it is explicitly trusted. An unrecognised `identity_kind` read back from the
database is treated the same way.

Windows enumerates volumes by walking `FindFirstVolumeW`/`FindNextVolumeW` and
resolving each GUID's mount points, rather than scanning drive letters A–Z.
That also finds volumes mounted into a folder, which have no letter at all.

## Concurrency

One `tokio` runtime. The scheduler owns a run queue, with at most one run per
destination volume so two rules cannot interleave writes on one disk. Within a
run, walking and copying use a bounded worker pool — around four for mechanical
disks, higher for SSDs, and separately lower for cloud-hydrating sources, since
oversubscribing a mechanical external drive makes it slower rather than faster.

Progress reaches the UI as events throttled to ten a second, so a million-file
run does not flood the IPC channel. Every run carries a cancellation token
checked between files.

IPC commands are synchronous, because each is a short database read or write.
The engine's long work never goes through them.

## Data model

`crates/shelv-core/migrations/0001_initial.sql` is the authority. Points worth
knowing:

- `PRAGMA foreign_keys` is set per connection. SQLite defaults it **off**, and
  without it every `ON DELETE CASCADE` silently does nothing.
- A `CHECK` enforces that `retention_kind` and `retention_value` are both set
  or both null. A half-written pair would read back as "keep everything" and
  quietly stop pruning snapshots.
- Enum columns are constrained to their known spellings. Those strings are an
  on-disk format: renaming one without a migration orphans existing rows, and a
  test round-trips every value.
- Unrecognised values fail towards safety. An unknown `drive_type` reads as
  `Unknown`, which is refused.
- Opening a database from a newer schema version is refused rather than written
  to with an older understanding of it.

## Types across the boundary

`src/types/` is generated from the Rust by `ts-rs` and committed, so the
frontend builds without a Rust toolchain. `scripts/check-generated-types.sh`
regenerates and fails if the result differs from what is committed.

Two details that bite:

- Numeric fields carry an explicit `#[ts(type = "number")]`. `serde_json`
  writes `i64` and `u64` as JSON numbers, so `JSON.parse` yields a `number`;
  ts-rs would otherwise emit `bigint`, describing a wire format that never
  occurs.
- Id newtypes implement `Serialize`/`Deserialize` by hand rather than using
  `#[serde(transparent)]`, which ts-rs cannot parse and warns on.

## Portability

Linux is a future target, so these are settled now rather than retrofitted:

- All platform code sits behind a trait, with the boundary enforced in CI.
- Volume identity is opaque, so no schema migration is needed for Linux.
- Paths are `Path`/`OsStr` throughout, never `String`. The `\\?\` long-path
  prefix is applied only inside `platform/windows.rs`, and by concatenating
  `OsString` rather than through a lossy conversion — Windows paths are UTF-16,
  and an unpaired surrogate would not survive the round trip.
- No assumption about case sensitivity. NTFS ignores case, ext4 does not, so
  the diff compares through `CaseSensitivity::fold`.
- mtime is the only metadata guaranteed across platforms. NTFS ACLs are
  explicitly not preserved.
- OneDrive has no official Linux client, so `cloud/` is Windows-only and its
  trait methods are a no-op elsewhere.
- CI builds and tests on Linux on every push.

## UI

React 19, TypeScript in strict mode, Vite, Tailwind v4, TanStack Table v8.

v8 is pinned deliberately: v9 is a redesign around atoms and explicit feature
composition, and v8 is sufficient for a table of this shape.

Colour lives in `src/styles/theme.css` as custom properties and nowhere else.
`scripts/check-contrast.mjs` reads that file, composites each translucent chip
fill over its surface, and fails CI below the WCAG minimums. Status is never
carried by colour alone — every state renders an icon and a word.

## Where to start reading

1. `crates/shelv-core/src/platform/mod.rs` — the traits everything else is
   written against.
2. `crates/shelv-core/migrations/0001_initial.sql` — the data model.
3. `crates/shelv-core/src/view.rs` — what the UI is handed.
4. `src-tauri/src/commands.rs` — the whole IPC surface.
5. `src/components/RuleTable.tsx` — the main screen.

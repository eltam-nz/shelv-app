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
| `scheduler/` | Whether a rule is due. The queue and run-on-connect are **M3.2**. |
| `safety/` | Path canonicalisation and the destructive-operation guards. |
| `civil/` | Calendar arithmetic: instants to dates, and the periods a schedule counts in. |
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
availability and how many **rules** use it — distinct rules, counted through
a `UNION`, not appearances. A rule copying one folder on a drive to another
folder on the same drive touches it twice, and adding those up reads as
"2 rules" when there is one. Attached-but-unrecorded drives are
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

### One run, start to finish

`engine::run::execute` is the order those three modules go in: plan, copy,
and — for a mirror only — move what the source no longer has out of the way.
Deletion comes **after** the copy, so a file about to be replaced is
replaced rather than trashed and rewritten, and a run cancelled part-way
through its copy never reaches the deletion pass at all.

Two decisions live here rather than in any of the three:

- **Where a run writes.** A mirror writes into the destination folder
  itself. A snapshot writes into a new timestamped folder beneath it and
  never touches one already there — no diff, no deletion, every file copied
  again. A snapshot that skipped unchanged files would be a snapshot with
  holes in it, and restoring from it would need every earlier folder. If a
  second run starts within the same second as one that already exists, it
  takes the next free name rather than writing into it; one-second timestamp
  resolution is otherwise a way for "nothing already written is ever
  touched" to hold for last year's snapshot and not for the one from a
  moment ago.
- **How the run is recorded.** Cancelled beats everything, because a
  cancelled run has neither failed nor succeeded and putting it in the
  `Partial` column would hide the runs that need attention. Otherwise any
  unreadable file, failed copy or failed move makes the run `Partial`;
  only a run that did everything it planned is `Ok`.

Retention is M3, so snapshots accumulate. The editor already says pruning is
governed by the retention setting, which must not be read as saying it is
happening yet.

### Backup Now

The engine is synchronous and knows nothing of Tauri. `src-tauri/src/runner.rs`
is what gives it a thread, turns its callbacks into events and lets the window
stop it.

- **One run at a time**, app-wide. Two runs could otherwise write to one drive
  at once, and a second Backup Now on a rule already running would copy the
  same tree twice. The scheduler in M3 will queue per volume; until then
  refusing the second run, and saying why, is the honest behaviour.
- **Its own database connection.** A backup takes minutes, and holding the
  lock the commands share for that long would freeze the rule table while it
  shows the backup running. `SQLite` is in WAL mode with a busy timeout, so a
  second connection is the ordinary answer; a run writes two rows, one at the
  start and one at the end.
- **Progress throttled to ten events a second.** A million-file run would
  otherwise spend its time serialising events. The status bar shows counts
  rather than a bar, because the totals grow as the run reaches each
  destination and a bar would appear to go backwards.
- **A preview before anything is removed.** A mirror deletes whatever its
  source no longer has — the behaviour that was asked for, and the one that
  destroys files when a rule points somewhere unintended. So Backup Now on a
  rule whose plan holds deletions shows them first, by path, with where they
  will go. A run that only copies starts immediately: a confirmation nobody
  needs is a confirmation nobody reads.
- **Each destination is its own history row**, because each succeeds or fails
  on its own. A drive that was unplugged is recorded as skipped, not as a
  failure — that is the ordinary case for a removable drive, and colouring it
  red would train someone to ignore the colour.

### When a rule is due

`scheduler::due` is a pure function — no store, no filesystem, no clock of
its own — so the question that starts an unattended backup is answerable in
a test as a table of dates.

**A schedule says how often, not when.** `Daily` means the last *successful*
run was on an earlier local calendar day; `Weekly` an earlier week,
`Monthly` an earlier month. There is no firing time. The alternative,
"daily at 02:00", needs a timezone database, a rule for the hour that
repeats each autumn and the one that never happens each spring, and
persisted next-run bookkeeping that has to survive the clock changing
underneath it — all to express something a backup does not need.

Three things follow:

- **Catch-up is arithmetic, not a feature.** A machine that was off for a
  week crossed six day boundaries; when it returns the rule is due. Nothing
  was missed, only delayed, so `catch_up` was dropped in migration 0004 —
  the same reasoning that removed `allow_deletions` once the layout answered
  its question.
- **No timezone database.** `LocalTime::utc_offset_seconds` asks the OS for
  the offset *at a given instant*, which handles daylight saving by
  construction because the OS knows what the offset was. It is a separate
  trait from `PlatformFs` because a clock is not a filesystem, and returns
  `0` rather than failing: a backup tool that refuses to run over a time
  zone it cannot read has chosen the worse harm.
- **Measured from the last success, not the last run.** A rule failing every
  night stays due rather than going quiet after its first attempt.

`Manual`, disabled and custom-schedule rules are never due, each with its
own reason, because the table has to say which. Cron is in the data model
and nothing evaluates it: treating it as daily would run a backup on a
schedule nobody chose, and treating it as manual would hide that the
setting does nothing.

### Refusing a run nobody is watching

M1 only ever deleted with somebody reading a preview. The scheduler removes
that person, and a mirror faithfully reproduces a source that has gone
missing by emptying the backup of it — a drive that mounted empty, a folder
renamed, a sync client that has not finished. `safety::deletion_refusal`
stands in for the reader who is not there.

An unattended run whose plan would remove **more than a quarter** of what is
at the destination, **and more than eight files**, stops. Both halves are
needed: a share alone refuses a two-file folder losing one, which is
ordinary; a count alone never triggers on a large backup, where losing a
quarter is exactly the disaster. Neither number is principled — a routine
day's deletions are a handful out of thousands, and the event this catches
takes nearly all of them, so a quarter sits far above one and far below the
other.

It stops **before anything is written**, copies included: a run that has
decided the source looks wrong must not half-apply itself, since the copies
come from the same reading of it. The outcome is `RunResult::Refused`, added
in migration 0005 — not `Failed`, because nothing is broken and a refusal in
the column someone checks for dying drives would be read as one. The reason
goes into the run's error text, which is what the history shows.

**Backup Now is never refused.** The preview is the guard there, and someone
who has read it and pressed the button has already made this decision with
the numbers on screen.

### Starting a run nobody asked for

`scheduler::sweep` reads the store and the attached volumes and returns the
rules that should start. It writes nothing and starts nothing, so what the
scheduler will do is testable without a scheduler.

There are two occasions to look, and they ask different questions:

- **Every fifteen minutes.** Periods are calendar days at the finest, so a
  tick this slow is still far finer than any of them, and it lets a laptop's
  disk stay asleep.
- **When a drive appears.** For an occasionally-connected drive this is the
  trigger that matters (`docs/PLAN.md` §1.1d). A rule set to run on connect
  backs up when the drive turns up **even if its period has not passed** —
  the drive is here now and may not be on the first of the month. A one-hour
  floor keeps a loose cable from queueing a run on every reconnection.
  Watching for a drive *appearing*, rather than reacting to any change, is
  what keeps unplugging one — or renaming it — from starting a backup.

A rule that is due but cannot reach its drives is **left out, not queued to
fail**: its moment comes when the drive appears. Anything that changes what
is due — a rule created or edited — nudges the scheduler rather than waiting
out the tick.

**Scheduled runs queue; manual ones do not.** The scheduler has nobody to
tell that a run was declined, so its rules wait their turn, deduplicated by
rule — a rule that is due *and* has just had its drive plugged in is one
backup. Backup Now has somebody watching, and a place in a line whose length
they cannot see is worse than an answer, so it is still refused while another
run is going. The queue moves on whatever happened to the run in front of it;
a failure must not strand the rules behind it.

The history records which occasion started a run: `schedule`, `on_connect`,
or `catch_up` when the rule had been due for more than one period. The run is
no different — the word is there so someone can tell why a backup happened on
a Tuesday afternoon.

### Pruning old snapshots

`engine::retention` is the only code in Shelv that deletes without a way
back. Mirror deletions move to a trash folder on the same volume, which
costs nothing and can be undone; pruning exists precisely to reclaim that
space, so a trash folder here would defeat the point. What guards it is that
every condition has to hold before a single folder goes:

- the run that just finished did **everything** it planned — not `Partial`,
  which may have failed to copy the very file an old snapshot is the last
  copy of (`docs/PLAN.md` §1.1c);
- the folder's name parses as one Shelv wrote, through
  `stamp::parse`, which refuses anything that is not exactly the shape
  `folder_name` produces — a destination may hold anything, and a directory
  that merely shares it is never touched;
- it is not the snapshot this run just made;
- and the rule asked for a limit at all.

`KeepLastN` counts the snapshot just written, so "keep the last 3" leaves
three. `KeepDays` measures from the folder's **name**, not its mtime, which
a copy tool has already touched. What was pruned is recorded in the run
history, because something that deletes and leaves no record is exactly what
the history is for.

The trash folders a mirror leaves are **not** pruned, by decision: they are
the recovery path for the mistake automation makes more likely, and an
automatic sweep of them would be the same mistake a month later. Clearing
them is manual.

### The tray, and the pause switch

Shelv runs as a **per-user tray process, never a Windows service**. That is a
correctness requirement before a convenience one: a service context receives
`STATUS_CLOUD_FILE_ACCESS_DENIED` instead of `OneDrive` hydration
(`docs/PLAN.md` §2), so the background half of a tool that backs up cloud
folders cannot be a service at all.

Closing the window therefore **hides** it. A backup tool that stops backing
up because somebody closed a window is not automated, and Quit lives in the
tray menu — which is also where it says it is still running. A machine with
no tray still works: failing to build one is logged, not fatal, and the
window is all that is lost.

**Pause holds every automatic backup**, from the tray or the header, and the
status bar says so — a backup tool that is not backing up has to admit it
somewhere always visible. It stops runs from *starting*; a run already going
is left to finish, since nothing is left half-written either way and the next
run would only repeat the work. Cancel is there for the stronger thing. It
is deliberately **not persisted**: a pause is a decision about this
afternoon, and coming back from a restart still silently paused is the worst
kind of quiet.

**Run at login is off until asked for**, in the About panel. The frontend
asks through a command taking a boolean, so `tauri-plugin-autostart`'s own
commands stay out of `capabilities/` and the id-only IPC surface is
unchanged.

**Notifications only when something needs a person**: failed, refused, or
partial. One after every successful backup is one nobody reads, and the
value of the refusal notice is that it interrupts.

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

Three things in the rule table are decided rather than default:

- **A drive's state shows beside its name**, as a glyph with the word
  carried in a `sr-only` span and in the title, not in a column of its own.
  `Available` renders nothing at all: nearly every row is available, and
  marking those too teaches the eye to skip exactly the marks that matter.
  The drives pane still spells every state out in full, which is where
  someone goes when a mark prompts a question.
- **Every mark is the blocked colour, `Disconnected` included** — grey in
  its pill, rose here. The two places answer different questions. The drives
  pane is an inventory, where a drive that is not plugged in is a neutral
  fact about that drive; the rule table answers "can this back up?", and
  there an unplugged destination is the rule not running. Grey would let it
  recede into the em dashes and italic paths around it. The glyphs and the
  hover text still separate "plug it in" from "this is the wrong disk".
- **The pinned actions column is set apart by surface, not by a rule.** It
  sits on `--surface` against the table's `--bg`, with an ordinary
  `--border` edge. It previously used `--border-strong`, which is sized for
  controls that must clear 3:1 and drew the eye to a border rather than to
  the rows.
- **Backup Now is a filled button; Edit Rule is not.** One of them writes to
  a disk. Disabled keeps the button's shape and drops the fill, so a row
  that cannot run still reads as a row that has the button.

The two buttons that *do* something rather than toggling something —
Backup Now and New rule — carry their hover state as `.btn-soft` and
`.btn-primary` in `global.css` rather than as inline styles, because an
inline style cannot have one. Each hover is a `color-mix` off the same
token, so it follows the palette instead of introducing a colour the
contrast check has never seen, and a disabled Backup Now stays inert: a row
whose rule cannot run must not light up under the pointer as though it
could.

**Next Backup answers the question it is named for.** It showed an em dash
from M0 until M3 precisely so it could not lie, and the same standard
applies now: "Due now", "Tomorrow", a weekday while there is only one it
could mean, a date beyond that, "Manual only", "Disabled", "Paused". The
answer is computed in Rust, because it is calendar arithmetic in the
machine's own time zone and a second implementation in TypeScript could
disagree about what day it is.

The one case worth the extra words is a rule that is due and cannot reach
its drives: it says **"When Archive 4TB is connected"** rather than "Due
now". A rule that is due but motionless reads as something being wrong;
naming the drive turns it into something waiting for you.

Deleting a rule lives in the editor's footer, at the opposite end from Save,
and opens a confirmation. Not in the table: a row already carries an action
per rule, and a third would put a destructive button beside Backup Now on
every line.

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

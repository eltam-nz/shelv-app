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
| `engine/` | Plan, execute, prune. **M1.** |
| `cloud/` | OneDrive hydration, budgets, pin-state restore. **M4.** |
| `scheduler/` | Cron evaluation, catch-up, run-on-connect, the run queue. **M3.** |
| `safety/` | Path canonicalisation and the destructive-operation guards. **M1.** |
| `volumes/` | Volume tracking and identity verification. **M1.** |

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

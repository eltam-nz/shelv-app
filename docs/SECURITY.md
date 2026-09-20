# Security

Shelv reads and writes people's files. The threats that matter are the ones
that destroy or expose data, not the ones that crash the app.

## What Shelv does not have

The strongest property of the design follows from its scope: **Shelv holds no
credentials.** It reads the local OneDrive folder on disk and relies on the
OneDrive client to fetch content; it never authenticates to Microsoft Graph or
anything else. No OAuth, no token store, no refresh flow, nothing to steal.

This is a binding constraint, not a coincidence. Adding any remote destination
destroys it and requires this document to be rewritten.

There is also no telemetry, no analytics and no crash reporting. The only
outbound request Shelv will ever make is the update check against GitHub, and
that will be disableable. Note that while the repository is private this is a
matter of trust rather than something a user can verify.

## Assets and boundaries

**Assets:** the user's source files — integrity above all, since a backup tool
that corrupts or deletes originals is worse than no backup tool — the backup
copies, and the rule database.

**Trust boundaries:** WebView ↔ Rust core; Rust core ↔ filesystem; app ↔
network; supply chain.

**Non-goal:** defending against an attacker already executing code as this
user. They do not need Shelv.

## Threats

| # | Threat | Mitigation | Status |
|---|---|---|---|
| T1 | A crafted filename rendered in the rule table achieves script execution, which then drives the backup engine | Strict CSP; no `dangerouslySetInnerHTML`; no `eval`; `withGlobalTauri: false`; `freezePrototype`. Decisively, **the frontend holds no filesystem capability** — commands take ids and Rust resolves paths from the database | Implemented, tested |
| T2 | Path traversal or a symlink escaping the source tree | Canonicalise both ends; reject any resolved path that is not a descendant of the rule root; `symlink_metadata` and do not follow links by default; depth cap; cycle detection | M1 |
| T3 | Writing a backup to the wrong disk after a drive letter is reassigned | Match on stable volume identity before any write; a mismatch is surfaced as "Different drive". Where no stable identity exists at all, the volume is marked unverifiable and is equally unwritable, so "could not identify" never degrades into "assume it is the right one" | Implemented, tested |
| T4 | A mirror rule deletes the user's data | Deletions off by default and opt-in per rule; refuse destination inside source and the reverse; refuse drive roots and system directories; dry run shows the deletion count first; delete to the recycle bin; temp file plus atomic rename so an interrupted run never truncates a good copy | M1 |
| T5 | Cloud hydration fills the system disk, or releases a file the user wanted kept locally | Preflight space check on both volumes; bounded hydrate-copy-release batches; per-rule budget; record each file's prior pin state and release only files Shelv itself hydrated | M4 |
| T6 | Hydration produces truncated stubs that look like successful backups | Never memory-map a sync root; per-file hydration timeout; verify size against the placeholder's logical size before counting the copy as successful | M4 |
| T7 | Data exfiltration | No telemetry, no analytics, no crash reporting; the updater is the only outbound request and is disableable | Implemented (nothing to disable yet) |
| T8 | A malicious update achieves code execution | Updater verifies a minisign signature against a public key compiled into the binary; the private key lives only in Actions secrets; releases are built by tagged CI and drafted for human confirmation | M5 |
| T9 | Supply chain compromise | `cargo-deny` and Dependabot in CI; lockfiles committed; actions pinned by commit SHA; a deliberately small dependency surface | Implemented |
| T10 | Privilege escalation | Runs strictly as the logged-in user. No elevation, no Windows service, no admin installer | Implemented |
| T11 | Rule database tampering or disclosure | Per-user directory with default ACLs. Paths and metadata only — no credentials, because none exist | Implemented |
| T12 | A locked file, such as an open Lightroom catalog, copied in a torn state | Detect the sharing violation, record the file as skipped, mark the run **Partial**. Never report success on a torn copy | M1 |

## Keeping Shelv off network shares

Requirement: local disks and physically connected drives only. Two layers,
both re-checked immediately before the first write of every run, because
configuration-time-only validation is bypassable by anything that edits the
database.

1. **Drive-type gate.** `GetDriveTypeW` must return `DRIVE_FIXED` or
   `DRIVE_REMOVABLE`. `DRIVE_REMOTE` — SMB and NFS shares, mapped letters, UNC
   paths — plus `DRIVE_CDROM` and `DRIVE_RAMDISK` are refused. Anything
   unclassifiable is refused too: `DriveType::is_permitted` defaults to no.
2. **Tauri capabilities.** Only `core:default`, `dialog:allow-open` and
   `notification:default` are granted. The `fs` and `shell` plugins are not
   enabled for the frontend at all.

## How the frontend's lack of filesystem access is enforced

`tauri-plugin-fs` **is** in the dependency graph — `tauri-plugin-dialog` pulls
it in so a picked path can be added to the fs scope. Its absence from
`Cargo.toml` was therefore never the guarantee. Three independent layers are,
and each was verified by planting the corresponding regression and confirming
it is caught:

1. **Build time.** Tauri's build script refuses to compile a capability naming
   a permission whose plugin is not a dependency. Adding
   `fs:allow-read-text-file` fails the build outright.
2. **Run time.** The ACL denies any command no capability grants. The tests in
   `src-tauri/src/security_tests.rs` register `tauri-plugin-fs` *deliberately*
   and show its commands are still refused — registering a plugin grants
   nothing by itself.
3. **Review.** An explicit `EXPECTED_PERMISSIONS` list catches what the other
   two miss: a permission that is valid and would compile, but that nobody
   reviewed.

One limitation, recorded so the tests are not mistaken for something stronger:
`mock_context` embeds no capabilities at all — `notification:default` is
granted by the real manifest yet is still refused under the mock. The runtime
tests therefore demonstrate Tauri's deny-by-default behaviour, not the
correctness of the shipping manifest; the manifest tests carry that half.
`assert_the_mock_grants_nothing` pins this and will fail if a future Tauri
starts loading real capabilities into the mock, at which point the runtime
tests can be strengthened.

## Volume identity

Getting this wrong loses data rather than merely annoying someone, so it is
worth stating how the refusal works.

A volume is trusted only if it carries an identity that survives being
unplugged and reattached: a volume GUID path on Windows, a filesystem UUID on
Linux. Where the system offers neither, the volume is recorded as
`VolumeIdentityKind::Unverified` and refused — it cannot be told apart from a
different drive appearing in the same place.

Nothing tests for a specific identity variant. Every decision goes through
`VolumeIdentityKind::is_stable`, so a scheme added later is refused until
someone explicitly marks it trustworthy, and an `identity_kind` read back from
the database that this build does not recognise is treated as unverifiable
rather than assumed to be one we trust.

## Deliberate exclusions

**Archive encryption.** Legacy ZipCrypto is cryptographically broken and will
never be offered. AES-256 zip or `age` would be sound but bring key management
and an unrecoverable-data failure mode that v1 does not need. For a
destination drive that needs protection, BitLocker To Go ships with Windows
and is the right answer.

**Volume Shadow Copy**, which would allow copying locked files, is deferred to
v1.1. Until then a locked file is reported as skipped and the run is marked
Partial.

## Reporting a vulnerability

The repository is private. Raise anything you find directly with the owner.

If it becomes public, this section should be replaced with a disclosure address
and a response-time commitment, and GitHub private vulnerability reporting
should be enabled.

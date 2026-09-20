# Contributing to Shelv

## Prerequisites

- Rust (stable, 1.82+) and `cargo`
- Node 22+ and `pnpm`
- `cargo install cargo-deny --locked`
- Linux builds additionally need: `libwebkit2gtk-4.1-dev libgtk-3-dev
  libayatana-appindicator3-dev librsvg2-dev patchelf`
- To cross-check the Windows code from Linux: `gcc-mingw-w64-x86-64` and
  `rustup target add x86_64-pc-windows-gnu`

```sh
pnpm install
```

## Checks

Every one of these runs in CI and must pass before merge.

| Check | Command |
|---|---|
| Rust formatting | `cargo fmt --all --check` |
| Rust lints | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust tests | `cargo test --workspace` (see the Windows note below) |
| Dependency audit | `cargo deny check` |
| TypeScript lints | `pnpm lint` |
| TypeScript formatting | `pnpm format:check` |
| TypeScript types | `pnpm typecheck` |
| Frontend tests | `pnpm test` |
| Frontend build | `pnpm build` |
| Platform boundary | `./scripts/check-platform-boundary.sh` |
| Generated types | `./scripts/check-generated-types.sh` |
| Palette contrast | `pnpm check:contrast` |
| Windows cross-check | `cargo clippy -p shelv-core --target x86_64-pc-windows-gnu --all-targets -- -D warnings` |

`pnpm format` rewrites files in place; `cargo fmt --all` does the same for Rust.

## Generated types

`src/types/` is generated from the Rust types that cross the IPC boundary and
is committed, so the frontend builds without a Rust toolchain. After changing
any of those types:

```sh
./scripts/gen-types.sh
```

CI runs `check-generated-types.sh`, which regenerates and fails if the result
differs from what is committed. Drifting IPC payload shapes are the most common
bug in a Tauri app and the hardest to spot, because both sides still compile
and the mismatch only appears at runtime.

Do not hand-edit anything under `src/types/` — the generator overwrites it.

## Running the app

```sh
pnpm tauri dev          # dev server + window
pnpm tauri build        # release installer (Windows targets)
```

On a headless Linux machine the window can still be exercised:

```sh
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 ./target/release/shelv
```

## Lint policy

The workspace denies `unwrap`, `expect`, `panic`, indexing and lossy numeric
casts (see `[workspace.lints]` in the root `Cargo.toml`). This is deliberate:
Shelv moves and deletes people's files, and a panic mid-run is a corrupted
backup. Handle the error, or document in the code why the case is impossible.

`clippy.toml` additionally bans `Path::to_string_lossy` and `std::fs::copy` —
the first mangles non-UTF-8 paths, the second skips the temp-and-rename
discipline that keeps an interrupted run from truncating a good backup.

`cargo deny` reports unmaintained crates only for our own direct dependencies,
since Tauri's transitive tree carries several we cannot act on. Vulnerabilities
are denied wherever they appear.

## Cross-checking Windows from Linux

`src-tauri` and the Win32 layer in `shelv-core` only compile for Windows, and
most day-to-day development here happens on Linux. Typecheck them without a
Windows machine:

```sh
cargo clippy -p shelv-core --target x86_64-pc-windows-gnu --all-targets -- -D warnings
```

This is worth the setup. It caught a bug that would otherwise have shipped: the
Win32 `DRIVE_*` values are bare `u32` constants, not a newtype, so writing them
as `match` arms made each one an irrefutable binding that matched every value —
every drive, network shares included, would have classified as `Fixed` and the
drive-type gate would have passed everything it exists to block.

CI builds the real `x86_64-pc-windows-msvc` target on `windows-latest`; the GNU
triple is a local convenience, not a release target. It is used rather than
MSVC here because `rusqlite`'s bundled SQLite needs a C compiler, and mingw
cross-compiles where the MSVC toolchain is unavailable.

## Colour

`src/styles/theme.css` holds every colour as a CSS custom property. Nothing
else defines one — components reference the tokens, and Tailwind utilities are
mapped onto the same tokens in `global.css`, so there is a single place to
verify.

`pnpm check:contrast` reads that file, composites each translucent chip fill
over its surface, and computes WCAG 2.1 ratios:

- **4.5:1** for text (1.4.3)
- **3.0:1** for a boundary that identifies a control or its state (1.4.11) —
  input borders and the focus ring
- **1.3:1** for a purely decorative divider. 1.4.11 does not reach these, since
  the table is delineated by spacing and content rather than by gridlines, but
  an invisible divider is still a bug.

Pastels on a dark background are easy to get wrong: they look fine on a good
monitor and fail for everyone else. Do not eyeball them — add the token and run
the check.

**Colour is never the only carrier of meaning.** Every status renders an icon
and a word as well as a hue, so the table stays legible in greyscale and to a
colour-blind reader. Tags store a palette token such as `pastel-blue`, never a
raw colour, which is what lets the theme guarantee readability.

## Repository conventions

- `main` is protected. Changes land through a pull request with CI green.
- Squash merges, so `main` reads as one commit per change.
- [Conventional Commits](https://www.conventionalcommits.org/) for the subject
  line: `feat:`, `fix:`, `docs:`, `chore:`, `test:`, `build:`, with an optional
  scope such as `feat(core):`.
- Releases are tagged `vX.Y.Z`, which is what triggers the release workflow.

### Dependencies

Dependabot watches cargo, npm and GitHub Actions weekly. Actions are pinned by
commit SHA — a tag is mutable and can be repointed at different code — so
Dependabot is the only thing that moves them, and without it a security fix
would never arrive.

`@tanstack/react-table` is held at v8 deliberately and major updates are
ignored; see the task #9 commit for why.

### Nothing may assume the repository stays private

It is private today and may not be later (`docs/PLAN.md` §4.4). So:

- no real paths from a developer's machine in fixtures, tests or documentation;
- no keys, tokens or credentials, including in commit history, where they
  would survive deletion of the file;
- no personal identifiers.

Shelv has no credentials of its own to leak — it never authenticates to
anything — but the repository still must not acquire any.

## Running tests on Windows

`cargo test --workspace` fails on Windows in the `shelv` crate with
`STATUS_ENTRYPOINT_NOT_FOUND`, before any test runs. This is not a bug in the
tests.

Anything linking Tauri has `WebView2Loader.dll` as a load-time import. Tauri's
build script copies that DLL next to the main binary in `target\debug\`, but
test binaries are built into `target\debug\deps\`. The test binary resolves
a different loader from `PATH` instead, whose exports do not match, and the
process dies during DLL initialisation.

On Windows, run `cargo test -p shelv-core`, which is what CI does. Nothing is
lost: every platform-specific line lives in `shelv-core`, and the shell crate's
tests exercise Tauri's ACL and parse JSON config, neither of which varies by
platform. They run on Linux, and `cargo clippy --all-targets` still compiles
them on both, so a broken test fails the build either way.

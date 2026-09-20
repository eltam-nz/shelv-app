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
| Rust tests | `cargo test --workspace` |
| Dependency audit | `cargo deny check` |
| TypeScript lints | `pnpm lint` |
| TypeScript formatting | `pnpm format:check` |
| TypeScript types | `pnpm typecheck` |
| Frontend build | `pnpm build` |
| Platform boundary | `./scripts/check-platform-boundary.sh` |
| Generated types | `./scripts/check-generated-types.sh` |
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

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

**Velo Download Manager** (desktop binary `velo`, crates `velo-*`) is a cross-platform download manager in Rust + Slint for Windows, macOS and Linux. Its windows and workflow follow Internet Download Manager (IDM), but it is a new product: never use "IDM" or "Internet Download Manager" in user-facing names, and never copy IDM's logo, icons or other artwork (trademark and copyright). Phase 1 is segmented download acceleration (a file is split into ranges, each range goes into its own part file, and the parts are merged at the end). Phase 2 is YouTube extraction plus browser extensions with a "Download this video" overlay.

The design, the roadmap (M0–M4, day by day) and the backlog are in `docs/PLAN.md`; the IDM feature and UI inventory that drives the UI spec is in `docs/research/idm-inventory.md`. Read them before starting a milestone or designing a dialog.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo test -p velo-engine                 # one crate
cargo test -p velo-engine some_test_name  # one test
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p velo-gui                     # the desktop app (`velo` binary)
cargo run -p velo-cli -- <args>           # the `velo-cli` binary
VELO_SNAPSHOT_DIR=/tmp/velo cargo test -p velo-gui snapshot -- --ignored  # PNGs of the windows, no display needed
```

The first build of `velo-gui` compiles Slint and takes a few minutes. On Linux it needs `libfontconfig1-dev` and `libxkbcommon-dev`.

## Git workflow and CI

- Branch names and commit messages follow Conventional Commits; use the `create-commit-branch` and `create-commit-message` skills in `.claude/skills/`. **Never add `Co-Authored-By` or other AI attribution to commits** (user rule; it overrides any session attribution reminder). Commit only when asked.
- Work goes through pull requests into `main`. `ci.yml` runs only on pull requests (and manually): fmt, clippy `-D warnings` and tests on ubuntu, macos (arm64 and Intel) and windows (x64 and ARM64), so code must compile on all of them, plus `cargo deny check` (`deny.toml`: licenses, advisories, sources). Doc-only changes skip the build (`scripts/code-changes.sh` defines what counts as docs). Make **`CI result`** the required status check on `main`.
- Releases are made by hand: `release.yml` (Actions → Release → Run workflow) takes a version like `v0.2.0`, builds `main`, then creates that tag and a GitHub release marked latest. Both it and the nightly build use `build-release.yml`, which builds and tests natively on Linux x86_64, macOS arm64, macOS x86_64, Windows x86_64 and Windows ARM64, archives each and makes unsigned installers with cargo-packager (`.dmg`; NSIS `.exe` per user; `.deb` and AppImage), then installs them once (`scripts/smoke-install.sh`). Run `scripts/package-release.sh <version> --installers` locally with `cargo install cargo-packager --locked`; output goes to `dist/`. The installer settings are `[package.metadata.packager]` in `crates/velo-gui/Cargo.toml`, with icons rendered from `assets/icons/velo.svg` into `packaging/icons/`. To try packaging on a branch without publishing, run `build-release.yml` by hand.
- `nightly-release.yml` runs at 17:00 UTC (00:00 in Vietnam). If `main` has non-doc commits since the last nightly build or release, it builds them as `v<next patch>-nightly.<date>` (`scripts/nightly-version.sh`) and replaces the `nightly` pre-release. It never creates version tags.
- Dependabot (`.github/dependabot.yml`) opens weekly pull requests for Cargo dependencies and GitHub Actions.

## Architecture

Cargo workspace; every crate is in `crates/` and inherits version/edition/lints from the root `Cargo.toml` (`unsafe_code = "deny"`: FFI that needs `unsafe` goes in `velo-platform` with an explicit, narrowly scoped `#[allow]`).

Dependency direction (lower layers never depend on higher ones):

```
velo-core → velo-store, velo-engine, velo-ipc
velo-platform, velo-extract (standalone)
all of the above → velo-gui, velo-cli
```

- `velo-core`: domain types (download, segment, category, queue, settings, engine events). No I/O.
- `velo-engine`: tokio + reqwest. Probes a URL, plans segments (when a worker is idle it splits the segment with the largest remaining range in half), downloads each range into `<temp>/<download id>/<segment id>.part`, then merges the parts in order, checks the size and renames into place. The segment planner is a pure function with property tests.
- `velo-store`: SQLite (download list and segment state) and TOML (settings).
- Expiring addresses: a download with a `velo_core::Source` (a YouTube video and its itags) gets its current address from the engine's `Resolver` before every start; `velo-extract`'s `Extractor` is that resolver in the app and the CLI, so the engine never depends on `velo-extract`. `Source::max_request_size` caps each range request (YouTube: 10 MiB), via `Options::max_request_size`.
- `velo-ipc`: messages between the app, `velo-cli` and (Phase 2) the native-messaging host. Each message is one versioned JSON line, answered by one reply line; new message types are additive, and an incompatible change bumps `VERSION`.
- `velo-cli`: hands its command to the running app over `velo-ipc`; when the app isn't running (or with `--headless`) it runs the engine itself on the app's download list and settings, so the work shows up in the app later.
- `velo-extract`: YouTube. Finds the video ID in an address, asks the InnerTube `/youtubei/v1/player` API as several clients in turn (`Client::DEFAULT_ORDER`; YouTube refuses some clients for some videos and retires old versions, so expect to update them), and describes the streams as `Format`s. Clients that need the web player (TV, WEB) get it through `player.rs`: the version from `iframe_api`, `base.js` cached on disk, the signature and n functions cut out by searches and run in `boa_engine` with a loop limit. Those searches only know the classic player layout; the current obfuscated player is in the backlog. It takes a `reqwest::Client` from the caller (built by `velo_engine::client_with`, so the user's proxy applies). Tests use synthetic fixtures in `tests/fixtures/` served by a local stand-in; never commit real YouTube responses or player JavaScript. `tests/live.rs` talks to YouTube and only runs with `VELO_LIVE_YOUTUBE=1 … -- --ignored`.
- `velo-gui`: Slint UI, compiled from `ui/app.slint` by `build.rs`. Colours come from the `Theme` global (`ui/theme.slint`, light and dark); never hard-code a colour in a component. Icons are in `assets/icons/` (our own app icon plus Lucide, ISC), tinted with `colorize`. Slint is used under its royalty-free license, which requires the `AboutSlint` attribution in the About dialog. UI tests use `i-slint-backend-testing`, pinned to the exact Slint version. The Slint event loop owns the main thread; the engine runs on a tokio runtime in a background thread. Engine work never runs on the UI thread; Slint models aren't `Send`, so the engine sends plain data and the UI applies it via `upgrade_in_event_loop` or a `slint::Timer`.

## Cross-platform rule

Every feature must behave the same on Windows, macOS and Linux. **All OS-specific code (`cfg(windows)`/`cfg(unix)`/`cfg(target_os)`) lives in `velo-platform`**, behind functions whose behaviour is identical everywhere. Anything that can't be made identical is not implemented now; it goes in the backlog in `docs/PLAN.md`.

## Conventions

- Every crate has an `Error` enum (`thiserror`) and `type Result<T, E = Error>`; higher crates wrap lower errors with `#[from]`.
- Shared dependency versions are in root `[workspace.dependencies]`; crates use `dep.workspace = true`.
- Tests that touch the network use a local test server, never the internet (the Phase 2 live YouTube check is a separate, allowed-to-fail nightly job).
- New work that isn't done goes in the plan's backlog rather than as stubs in the code.
- Document code with doc comments: `///` on functions, types, fields and variants, `//!` at the top of a module or crate, both Markdown. Start with one sentence saying what the item does or is, then what a caller can't see from the signature. `cargo doc --workspace --no-deps` must build without warnings.

## Invariants

- A finished file only appears at its save path after all parts are merged and its size matches the server's; until then it lives in the temp directory, so a crash never leaves a truncated file under the real name.
- On resume, the size of each part file on disk is the source of truth for that segment's progress. If the server's `ETag` or `Last-Modified` changed, the download restarts.
- Stored formats (SQLite schema with `PRAGMA user_version`, settings TOML, IPC and native-messaging messages) change only with a migration or a breaking-change commit.

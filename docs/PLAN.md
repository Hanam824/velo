# Plan: "Velo Download Manager": a cross-platform IDM-style downloader in Rust + Slint

## Context
The goal is a cross-platform rewrite of Internet Download Manager (IDM) in Rust with a Slint UI. The windows and workflow follow IDM's layout. The name, logo and icons are our own: we must not ship IDM's trademark or artwork.
- **Phase 1:** segmented download acceleration. Each file is split into many ranges, each range is downloaded into its own part file, and the parts are merged when everything finishes.
- **Phase 2:** YouTube link extraction, plus a "Download this video" overlay on YouTube pages, delivered through browser extensions.
- **Starting point:** `/Users/tranthehuy/Documents/Github/IDM` is empty and not yet a git repo.
- **Borrowed setup:** CI/CD and `.claude` come from `Hanam824/neki`, which is already a Rust 2024 + Slint workspace.

**Decisions already made:**
- **Name:** working name "Velo", with crate prefix `velo-`. It is easy to rename later.
- **YouTube:** a **pure Rust extractor**.
- **Browsers:** Chrome/Edge/Brave (MV3), Firefox and Safari.
- **Release targets:** Windows x64 + arm64, macOS arm64 + x64, Linux x64.

---

## 1. Research inventory (store in `docs/research/idm-inventory.md`)
This section is a summary of the research findings. It drives the UI spec.

### Main window
**Menu bar:** Tasks, File, Downloads, View, Help.

**Toolbar**, in order, with large icons and labels underneath:
1. Add URL
2. Resume
3. Stop
4. Stop All
5. Delete
6. Delete Completed
7. Options
8. Scheduler
9. Start Queue ▾
10. Stop Queue ▾
11. Grabber
12. Share

**Left pane:** a "Categories" tree.
- All Downloads, containing Compressed, Documents, Music, Programs and Video.
- Unfinished, containing the same categories.
- Finished, containing the same categories.
- Grabber projects.
- Queues: Main download queue, Synchronization queue, and custom queues.

**Download list.** Columns can be sorted, reordered and hidden:
- File Name (with a file-type icon)
- Q
- Size
- Status (shows %)
- Time left
- Transfer rate
- Last Try Date
- Description

**Row context menu:**
- Open, Open With, Open Folder
- Move/Rename, Redownload, Resume, Stop
- Refresh download address
- Add to Queue / Delete from Queue
- Delete, Properties

**Tray icon:** closing the main window minimizes the app to the tray.

### Dialogs
- **Add URL:** URL box with history, plus "Use authorization" with user and password fields.
- **Download File Info:**
  - URL
  - Category with a "+" button
  - Save As with a "…" browse button
  - "Remember path for category" checkbox
  - Description
  - File icon and size
  - Buttons: Start Download, Download Later, Cancel
- **Progress dialog:**
  - Title: "NN% filename".
  - Three tabs: *Download status*, *Speed Limiter* and *Options on completion*.
  - Download status shows: Status, File size, Downloaded (x MB / yy %), Transfer rate, Time left, Resume capability.
  - A progress bar.
  - Buttons: Show/Hide details, Pause, Cancel.
  - Details section: a **segment bar**, where pink ticks mark segment starts, blue is downloaded and white is remaining.
  - A connections table with columns N. / Downloaded / Info (Connecting…, Send GET…, Receiving data…).
- **Download complete:**
  - Address, size, saved path.
  - Buttons: Open, Open with…, Open folder, Close.
  - "Don't show again" checkbox.
- **Properties:** name, location, URL, referer, description, login.
- **Options dialog tabs:**
  - General (integration, clipboard monitor, video panel settings)
  - File types (captured extensions and an exception site list)
  - Save To (folder per category, temp dir)
  - Downloads (dialog toggles, user agent, AV scan command with `[File]` placeholder)
  - Connection (default max connections 1/2/4/8/16/24/32, per-server exceptions, download limits)
  - Proxy/Socks
  - Site Logins
  - Sounds
  - Appearance (light/dark)
- **Scheduler:**
  - A queue list on the left.
  - A "Schedule" tab: one-time or periodic sync, start-at time with daily weekday checks, stop-at, retries, and on-completion actions (open file, exit, shutdown).
  - A "Files in the queue" tab: reorder files, and set "download N files at the same time".
  - Buttons: Start Now, Stop, Apply, Close, New/Delete queue.
- **Video panel:** a small dark semi-transparent bar at the top-right of the `<video>` element.
  - Clicking it opens a format list, with entries like "MP4 1280x720 (45.3 MB)".
  - The list also offers "Don't show on this site" and a close button.
- **Later-phase features:**
  - Site Grabber (4-step wizard)
  - Batch download (wildcard URL pattern)
  - Clipboard monitoring
  - Download limits per hour
  - Antivirus hook
  - CLI `/d URL /p path /f name /q /n /a /s`

### Segmentation algorithm (IDM's documented behavior)
1. Start with one connection.
2. Whenever a connection becomes free, find the segment with the **largest remaining range** and split it in half. The free connection takes the upper half, and the original segment's end shrinks to the midpoint.
3. Reuse keep-alive connections, so fast connections take over work from slow ones.
4. Stop splitting below a minimum segment size.
5. Save positions every few seconds so a download can resume.
6. Assemble the segments into one file when all are done.

---

## 2. Architecture

### Repository layout
This is a Cargo workspace using neki's conventions: `resolver = "3"`, edition 2024, `rust-version` 1.89+, `unsafe_code = "deny"`, clippy `all = warn`, release `lto = "thin"`, and shared versions in `[workspace.dependencies]`.

```
crates/
  velo-core/      domain types: Download, Segment, Category, Queue, Settings, events; thiserror Error
  velo-engine/    tokio + reqwest(rustls) engine: probe, segment planner, workers, part files, merge, resume, speed limiter
  velo-store/     persistence: rusqlite(bundled) for the download list and segment state; TOML for settings
  velo-platform/  the only crate with cfg(target_os): app dirs, open/reveal file, shutdown/sleep, autostart, notifications, tray, single-instance
  velo-ipc/       local-socket protocol (interprocess crate) between the GUI app, CLI and native-messaging host; serde JSON messages
  velo-gui/       Slint app, binary `velo`; ui/*.slint mirroring the IDM windows
  velo-cli/       binary `velo-cli`: IDM-style flags, talks to the running app over IPC, or runs headless
  velo-nmh/       binary `velo-nmh`: browser native-messaging host (stdin/stdout length-prefixed JSON ↔ velo-ipc)   [Phase 2]
  velo-extract/   YouTube extractor (pure Rust)                                                          [Phase 2]
extension/
  shared/ (TypeScript), chromium/ (MV3 manifest), firefox/ (manifest), safari/ (Xcode wrapper app)     [Phase 2]
assets/icons/     our own SVG icon set (no IDM artwork)
docs/             PLAN.md, research/, design/ (screen specs + mockups), adr/
scripts/, .github/workflows/, .claude/, CLAUDE.md
```

### Engine design (`velo-engine`)
**Probe**
- Send `HEAD`, and fall back to `GET` with `Range: bytes=0-0`.
- Read the total size, `Accept-Ranges`/206 support, the filename from `Content-Disposition` or the URL, `ETag` and `Last-Modified`.
- If the server doesn't support ranges, use a single connection and mark the download as not resumable.

**Segment model**
- Each segment is `Segment { id, start, end, downloaded, part_path, state }`.
- The segment list is kept sorted by `start`.

**Planner**
- When a worker is idle, it calls `split_largest(min_seg = 256 KiB)` (the minimum is configurable).
- This is a pure function so it can be unit-tested heavily.

**Workers**
- There are N tokio tasks, where N is the max connections setting (default 8, max 32, with per-host overrides).
- Each worker streams `Range: start+downloaded … end-1` into its own part file.
- If a segment's `end` shrinks while the worker is running, the worker stops at the new boundary. The shared end is an `AtomicU64`.
- On error, the worker retries with backoff. It reuses one `reqwest::Client` per host for keep-alive.

**Part files**
- Files are written as `<temp_dir>/<download_id>/<seg_id>.part`.
- When all segments are done, stream-concatenate the parts in `start` order into `<save_path>.velo-tmp`, then fsync, then rename atomically, then delete the parts. This gives the "split then merge" behavior that was asked for.
- Before renaming, check the final size against the total size.

**Persistence**
- Snapshot the segment state to SQLite every 2 seconds, and on pause or exit.
- On resume, each segment's offset is the size of its part file on disk, which is the source of truth.
- If the `ETag` or `Last-Modified` changed, restart the download (an option can override this).

**Other engine pieces**
- **Speed limiter:** a token bucket (global and per download), shared by all workers.
- **Queue and scheduler:** queues with a concurrency limit, start/stop times and retry counts. A tokio interval checks the schedules.
- **Events:** the engine sends `EngineEvent`s (progress snapshots, state changes) over a `tokio::sync::broadcast` channel. It exposes a command channel `EngineCmd` (Add, Pause, Resume, Delete, SetLimit…).

### GUI threading (adapted from neki's `crates/neki-gui/src/app.rs` `spawn_task` pattern)
- **Main thread:** the Slint event loop.
- **Tokio:** a multi-thread runtime runs on a background thread and owns the engine.
- **Progress updates:**
  - A `slint::Timer` fires every 250 ms and drains the latest progress snapshots, which are plain `Send` data.
  - It then updates `VecModel`s via `upgrade_in_event_loop`. Slint models are not `Send`, so they stay on the UI thread.
  - The segment bar and connections table only update for the progress dialogs that are open.
- **Segment bar:** a custom Slint component built from a `[SegmentView{start_frac, done_frac, end_frac}]` model and drawn with `Rectangle`s. The pink, blue and white styling uses our own palette.
- **Tray:** the `tray-icon` crate, initialised on the main thread. On macOS it needs the winit backend.

---

## 3. Borrowing from neki (Hanam824/neki)

### Copy and adapt
Rename neki → velo everywhere in these files.

**Workflows:**
- `.github/workflows/ci.yml`:
  - The docs-only filter using `scripts/code-changes.sh`.
  - A 4-OS matrix plus **add `windows-11-arm`**.
  - Steps: fmt, then clippy `-D warnings`, then tests.
  - The Linux Slint dependencies `libfontconfig1-dev libxkbcommon-dev`.
  - The single required check, "CI result".
- `build-release.yml` (reusable), `release.yml` (manual vX.Y.Z) and `nightly-release.yml` (the cron is `0 17 * * *` = UTC+7; keep it or adjust).

**Scripts:**
- `scripts/code-changes.sh`
- `scripts/nightly-version.sh`
- `scripts/release-notes.sh`
- `scripts/package-release.sh`: change the binary list to `velo velo-cli velo-nmh`.

**Config and Claude setup:**
- `rustfmt.toml`
- `.cargo/config.toml`: `MACOSX_DEPLOYMENT_TARGET` = 15.0 (as in neki), checked by `package-release.sh`.
- `.claude/skills/create-commit-branch/SKILL.md` and `create-commit-message/SKILL.md`: scopes become the crate names without `velo-`, plus `ext` and `ci`.
- `CLAUDE.md`: adapt neki's Commands, Git workflow, GUI threading rule, single-platform-crate rule and Conventions sections. Drop neki's Invariants and store sections.

**Drop:** `pkgs.yml`, `examples/` and `pkgs/`.

### Add (neki doesn't have these)
- **Installers:** `cargo-packager` in `build-release.yml`, producing:
  - Windows: NSIS `.exe` and `.msi` (x64 and arm64)
  - macOS: `.dmg` for arm64 and x64
  - Linux: AppImage and `.deb`
  - Keep neki's `.zip`/`.tar.gz` archives as well.
- **Code signing:** jobs that run only when the secrets exist (`APPLE_CERT`, `APPLE_NOTARY_*`, `WINDOWS_SIGN_*`). Until then, unsigned builds use neki's release-notes caveat.
- **Supply chain:** `deny.toml` checked with `cargo-deny`, and `.github/dependabot.yml` for cargo and GitHub Actions.
- **Extension job:** `npm ci && npm run build`, which zips the Chromium and Firefox extensions as release assets. Safari is built in the macOS job with `xcodebuild`.

**Open item:** neki's commit skill forbids `Co-Authored-By` lines, which conflicts with the session's attribution default. We need to decide which rule the new repo keeps.

---

## 4. Phase 2 design: YouTube

### `velo-extract` (pure Rust)
1. Parse the video ID from watch, shorts, youtu.be and embed URLs.
2. Call the InnerTube `/youtubei/v1/player` endpoint with several client profiles (ANDROID_VR / IOS / TV / WEB), falling back from one to the next. Collect `streamingData.formats` and `adaptiveFormats`.
3. For formats that use `signatureCipher` or need an `n` parameter:
   - Fetch `base.js`.
   - Extract the decipher and n-transform functions with regex.
   - Run them in an embedded JS engine (`rquickjs` or `boa_engine`).
   - Cache the results by player version.
4. Build a format list: itag, container, resolution, fps, codec, bitrate, `contentLength` and an audio/video flag.
5. **Muxing:** high-resolution streams are separate video and audio (DASH).
   - **First:** offer progressive formats (≤360p/720p) and adaptive downloads into separate files.
   - **Then:** add a pure-Rust MP4/WebM remux module (fMP4 box rewriting via the `mp4` crate / `matroska` writer).
   - **Fallback:** if pure-Rust remux slips, an optional `ffmpeg` found on PATH.
6. Downloads go through the normal segmented engine. YouTube throttles ranges, so we use about 10 MB chunks per request.
7. A **fixture-based test suite** (saved player responses and `base.js` files) plus a nightly live smoke test in CI that is allowed to fail and opens an issue. YouTube changes will break the extractor, so maintenance is expected.

**Risks:**
- PO-token and bot checks can block clients.
- The design is a pluggable client list with a "report broken" path.

### Browser integration
- **Extension (TypeScript, shared code):**
  - Content script on `*.youtube.com` that watches for `<video>` elements and SPA navigation (`yt-navigate-finish`).
  - Injects a shadow-DOM overlay button at the top-right of the player.
  - On click, sends `{type:"list_formats", url}` through native messaging, renders the format menu, and on a pick sends `{type:"download", url, itag, title, cookies?}`.
  - Also handles generic download capture (`chrome.downloads.onCreated` → cancel → hand off to Velo, filtered by the file-type list), the context menu "Download with Velo", and the Ctrl/Alt override.
- **`velo-nmh`:**
  - Speaks the stdin/stdout 4-byte length-prefixed JSON protocol.
  - Forwards to the running app over `velo-ipc`, and launches the app if it isn't running.
  - Installers register the host manifests:
    - Windows: the registry keys for Chrome, Edge and Firefox.
    - macOS and Linux: the `NativeMessagingHosts` directories.
- **Safari:** a Safari Web Extension inside a small macOS wrapper app (Xcode project in `extension/safari`).
  - The native side uses `SafariWebExtensionHandler`, which relays to the Velo app over a local socket.
  - It ships inside the macOS `.dmg`.
  - Built last, because it needs signing to be usable.

---

## 5. Day-by-day roadmap
Each day ends with a green CI run and a working build. Plan for about 1 developer-day per item; the sizes are estimates.

**M0: Skeleton and tooling (days 1–3)**
- D1:
  - `git init`.
  - Workspace `Cargo.toml` with all crates as empty stubs.
  - Copy neki's `rustfmt.toml`, `.cargo/config.toml`, `.gitignore` and LICENSE (MIT).
  - Write `CLAUDE.md`, the `.claude/skills` and `docs/research/idm-inventory.md`.
- D2:
  - Port neki's CI, release and nightly workflows and scripts, plus `deny.toml` and dependabot.
  - Push to GitHub and set "CI result" as the required check.
- D3:
  - `velo-gui` hello window (`build.rs` → `slint_build::compile("ui/app.slint")`).
  - Theme tokens (light/dark).
  - Icon set chosen (an MIT-licensed set, e.g. Lucide or Tabler, recolored).
  - The headless UI test harness (`i-slint-backend-testing`, pinned to the Slint version).

**M1: Engine core (days 4–10)**
- D4: `velo-core` types and errors. The `split_largest` planner with property tests (proptest: segments always tile `[0,total)` without overlap).
- D5: Probe (HEAD/Range), filename resolution, single-connection download to a part file.
- D6: Multi-worker segmented download with dynamic splitting and the atomic shrinking end.
- D7: Merge part files → verify size → atomic rename; cleanup; keep-alive client reuse; retry/backoff.
- D8: Pause, resume and persistence (`velo-store` SQLite schema, migrations); resume after process restart.
- D9:
  - Speed limiter (token bucket).
  - Per-host connection exceptions.
  - Auth (Basic), cookies, referer and user-agent headers.
  - Proxy (reqwest proxy, system proxy).
- D10: Integration tests against a local test server (`axum` or `hyper`) with a range-supporting endpoint, a no-range endpoint, a flaky-connection endpoint and a changed-ETag case. Benchmark against plain single-stream downloads.

**M2: IDM-style UI (days 11–20)**
- D11: Main window shell: menu bar, toolbar (12 buttons, enabled per selection), splitter, categories tree, status bar.
- D12: Download list `StandardTableView` (or a custom table) with the columns, sorting, file-type icons and context menu.
- D13: Wire the engine ↔ GUI bridge (tokio thread, `slint::Timer` snapshot pump, commands).
- D14: Add URL dialog + Download File Info dialog (category auto-detect by extension, save-path memory).
- D15: Progress dialog: status tab, progress bar, **segment bar component**, connections table, Show/Hide details.
- D16: Progress dialog tabs Speed Limiter and Options on completion; Download complete dialog; Properties dialog.
- D17: Options dialog with the tabs General, File types, Save To, Downloads, Connection, Proxy, Site Logins, Sounds, Appearance, persisted to TOML.
- D18: Categories: filtering and Unfinished/Finished views, plus add/edit/delete category.
- D19: Queues + Scheduler dialog (queue list, Schedule tab, Files in the queue tab, concurrency).
- D20: Tray icon, minimize-to-tray, single instance (velo-ipc), notifications, sounds, open/reveal file (velo-platform).

**M3: Release v0.1 (days 21–24)**
- D21: `velo-cli` (`velo-cli -d URL -p dir -f name -q -n -a -s`) talking over IPC or running headless.
- D22: `cargo-packager` installers in `build-release.yml` for all 5 targets; smoke-test that each installs.
- D23: Clipboard monitoring, batch download (wildcard pattern), import/export of the list.
- D24: Bug bash, docs, then tag `v0.1.0` through the `release.yml` workflow.

**M4: Phase 2, YouTube (days 25–38)**
- D25–26: `velo-extract`: ID parsing, InnerTube player call with multiple clients, format model, fixtures.
- D27–28: `base.js` signature and n-transform with an embedded JS engine; caching; fixture tests.
- D29: Engine support for chunked range requests against YouTube; progressive formats downloadable end to end from the Add URL dialog (paste a YouTube URL → format picker).
- D30–31: Pure-Rust DASH remux (MP4 video + M4A audio → MP4), with an optional ffmpeg fallback.
- D32: `velo-nmh` + IPC messages + host-manifest registration in the installers.
- D33–34: Chromium MV3 extension: download capture, context menu, YouTube overlay button + format menu.
- D35: Firefox build (manifest differences, `browser_specific_settings`, NMH manifest).
- D36–37: Safari Web Extension wrapper app + handler → IPC; included in the macOS dmg.
- D38: Release v0.2.0; nightly live-YouTube smoke workflow.

**Later (backlog):**
- Site Grabber wizard, download limits per hour, antivirus hook and synchronization queues.
- Generic HLS/DASH sniffing on non-YouTube sites.
- FTP, auto-update and localization.
- Toolbar buttons Grabber (with the Site Grabber) and Share (needs a project page to share).
- Saved logins (Options → Site Logins, and Add URL credentials that survive a restart): needs the OS keychain so passwords are never stored in plain text. Until then credentials live in memory only.

---

## 6. Verification
- **Every PR:** CI runs `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -D warnings`, `cargo test --workspace --locked` and `cargo deny check` on win/mac/linux (+ win-arm).
- **Engine:**
  - Unit and property tests for the planner.
  - Integration tests against the local axum server: correct bytes (sha256), resume after kill, no-range fallback, ETag change, speed-limit accuracy within ±10%.
  - Manual check: download a large public test file (e.g. a Linux ISO) with 1 vs 8 vs 16 connections and compare throughput.
- **UI:**
  - `i-slint-backend-testing` headless tests for the dialogs (button enable/disable logic, model updates).
  - `cargo run -p velo-gui` manual walkthrough against the research inventory checklist.
- **Release:** a nightly workflow produces installers; install each on a VM/runner and launch it (smoke step in CI that runs `velo-cli --version`).
- **Phase 2:**
  - Fixture tests for the extractor.
  - A nightly live test on a known public video (allowed to fail, but it alerts us).
  - Manual: load the unpacked extension in Chrome and Firefox and click the overlay on YouTube → the file downloads and plays.

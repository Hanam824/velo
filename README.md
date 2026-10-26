# Velo Download Manager

A cross-platform download manager for Windows, macOS and Linux, written in Rust with a [Slint](https://slint.dev) UI.

Velo splits each file into ranges that download in parallel over several connections, into part files that are merged when the download finishes. Downloads can be paused and resumed, also after Velo restarts.

Status: early development. See [`docs/PLAN.md`](docs/PLAN.md) for the design and roadmap, and [`CHANGELOG.md`](CHANGELOG.md) for what each release added.

## Features

- **Segmented downloads:** up to 32 connections per file. When a connection finishes its range, it takes half of the largest range still left. Downloads resume after a pause, a crash or a restart, and start over if the file changed on the server.
- **The main window:**
  - Categories (Compressed, Documents, Music, Programs, Video, plus your own), with All / Unfinished / Finished views.
  - A sortable download list.
  - A toolbar, a menu bar and a context menu.
  - Light and dark themes.
- **Dialogs:**
  - **Add URL:** optional login, held in memory only.
  - **Download File Info:** category, folder and description.
  - **Progress:** the segment bar, each connection, a speed limit, and what to do when the download completes.
  - **Download complete**, **Properties** and **Options**.
- **Speed limits:** a total limit and a per-download limit. Connection counts can be set per server.
- **Proxies:** system or manual (HTTP, HTTPS, SOCKS5). You can also set your own user agent.
- **Queues and the scheduler:**
  - Start and stop at set times on chosen days.
  - Run N downloads at a time.
  - When the queue finishes, quit Velo or turn off the computer.
- **Clipboard monitoring:** copy a link to a file type on the capture list and Velo offers to download it.
- **Batch downloads and lists:**
  - Batch: `https://example.com/photo*.jpg` from 1 to 100, zero-padded, or a to z.
  - Import and export the list as plain text.
- **The tray, single instance and notifications:** closing the window keeps Velo in the tray, and launching it again shows the running window.
- **`velo-cli`:** adds downloads to the running app, or downloads on its own (see below).

## Install

Download the file for your system from the [latest release](https://github.com/Hanam824/velo/releases/latest). Each release has an installer and a plain archive:

| System | Installer | Archive |
| --- | --- | --- |
| Windows x64 / ARM64 | `velo-<version>-windows-<arch>.exe` (installs for the current user, no admin rights) | `.zip` |
| macOS 15+, Apple Silicon / Intel | `velo-<version>-macos-<arch>.dmg` (drag Velo Download Manager to Applications) | `.tar.gz` |
| Linux x86_64 | `velo-<version>-linux-x86_64.deb` or `.AppImage` | `.tar.gz` |

The builds aren't signed or notarized yet:

- **macOS:** if macOS says the app is damaged or can't be checked, run `xattr -dr com.apple.quarantine "/Applications/Velo Download Manager.app"` once. You can also right-click the app and choose Open.
- **Windows:** SmartScreen may warn on the first launch. Choose *More info → Run anyway*.
- **Linux:** the archive and the AppImage need fontconfig and libxkbcommon. The `.deb` installs them itself.

## Command line

`velo-cli` hands its command to Velo if it is running. Otherwise it downloads by itself on Velo's download list, so what it downloads shows up in the app later.

```sh
velo-cli -d https://example.com/file.zip          # ask in Download File Info
velo-cli -d https://example.com/file.zip -n       # add without asking
velo-cli -d URL -p ~/Downloads/isos -f disk.iso   # choose the folder and the name
velo-cli -d URL -a                                # add to the main queue without starting
velo-cli -s                                       # start the main queue
velo-cli -d URL -q                                # download here, show progress, exit when done
velo-cli --help
```

`-q` (`--headless`) prints the saved file's path on success. It exits non-zero if the download fails. Ctrl+C pauses the download, and it can be resumed later.

## Build from source

```sh
./run.sh                # the desktop app
./run.sh --release      # optimized build
./run.sh cli --help     # the command-line tool
./run.sh test           # fmt, clippy and tests, as CI runs them
```

You need Rust (stable, from [rustup](https://rustup.rs)). On Linux, install `libfontconfig1-dev` and `libxkbcommon-dev` first. `./scripts/package-release.sh <version> --installers` builds the archive and the installers for your system into `dist/`. It needs `cargo install cargo-packager --locked`.

## License

MIT. Velo's UI uses [Slint](https://slint.dev) under its royalty-free license, and icons from [Lucide](https://lucide.dev) (ISC).

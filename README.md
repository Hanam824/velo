# Velo Download Manager

A cross-platform download manager for Windows, macOS and Linux, written in Rust with a [Slint](https://slint.dev) UI.

- **Phase 1:** segmented download acceleration. Each file is split into ranges that download in parallel into part files, which are merged when the download finishes.
- **Phase 2:** YouTube downloads and browser extensions for Chrome, Edge, Firefox and Safari.

Status: early development. See [`docs/PLAN.md`](docs/PLAN.md) for the design and roadmap.

## Build

```sh
./run.sh                # the desktop app
./run.sh --release      # optimized build
./run.sh cli --help     # the command-line tool
./run.sh test           # fmt, clippy and tests, as CI runs them
```

On Linux, install `libfontconfig1-dev` and `libxkbcommon-dev` first.

## License

MIT

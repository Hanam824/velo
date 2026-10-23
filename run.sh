#!/usr/bin/env bash
# Build and run Velo from source.
#
#   ./run.sh                  # the desktop app (debug build)
#   ./run.sh --release        # the desktop app, optimized
#   ./run.sh cli [args...]    # velo-cli, e.g. ./run.sh cli --help
#   ./run.sh test             # fmt check, clippy and tests, as CI runs them
#
# The first build compiles Slint and takes a few minutes; later runs are fast.
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v cargo > /dev/null; then
    echo "cargo not found: install Rust from https://rustup.rs" >&2
    exit 1
fi

# Slint needs fontconfig and xkbcommon on Linux.
if [ "$(uname -s)" = Linux ] && command -v pkg-config > /dev/null &&
    ! pkg-config --exists fontconfig xkbcommon; then
    echo "missing Slint libraries: sudo apt-get install -y libfontconfig1-dev libxkbcommon-dev" >&2
    exit 1
fi

case "${1:-}" in
    cli)
        shift
        exec cargo run -p velo-cli -- "$@"
        ;;
    test)
        cargo fmt --all --check
        cargo clippy --workspace --all-targets --locked -- -D warnings
        exec cargo test --workspace --locked
        ;;
    --release)
        exec cargo run --release -p velo-gui
        ;;
    "")
        exec cargo run -p velo-gui
        ;;
    *)
        sed -n '2,9p' "$0" >&2
        exit 2
        ;;
esac

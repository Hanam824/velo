#!/usr/bin/env bash
# Build a release of Velo for this machine's OS and architecture, and archive it for a GitHub release.
#
#   ./scripts/package-release.sh v0.1.0
#     -> dist/velo-v0.1.0-macos-arm64.tar.gz      (macOS, Linux: .tar.gz)
#     -> dist/velo-v0.1.0-windows-x86_64.zip      (Windows, from Git Bash: .zip)
#   ./scripts/package-release.sh v0.1.1-nightly.20261021   (a nightly build's version)
#   ./scripts/package-release.sh v0.1.0 --installers
#     also -> dist/velo-v0.1.0-macos-arm64.dmg        (macOS)
#             dist/velo-v0.1.0-windows-x86_64.exe     (Windows: NSIS installer, per user)
#             dist/velo-v0.1.0-linux-x86_64.deb       (Linux)
#             dist/velo-v0.1.0-linux-x86_64.AppImage  (Linux)
#
# The archive holds the Velo desktop app (`velo`), the `velo-cli` command-line tool, README.md and
# LICENSE. The installers are made by cargo-packager (`cargo install cargo-packager --locked`)
# from [package.metadata.packager] in crates/velo-gui/Cargo.toml, and are unsigned.
# Builds are native only: each platform is built on a machine of that platform. Stdout lists the
# files made, one per line, the archive first. The release version is written into Cargo.toml for the build (so
# `velo-cli --version` matches the tag) and Cargo.toml and Cargo.lock are restored afterwards.
set -euo pipefail

cd "$(dirname "$0")/.."

version="${1:-}"
installers=false
if [ "${2:-}" = --installers ]; then installers=true; fi
case "$version" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *)
        echo "usage: $0 <version tag, e.g. v0.1.0> [--installers]" >&2
        exit 2
        ;;
esac

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    MINGW* | MSYS* | CYGWIN*) os=windows ;;
    *)
        echo "unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac
case "$(uname -m)" in
    arm64 | aarch64) arch=arm64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *)
        echo "unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac
exe=""
if [ "$os" = windows ]; then exe=".exe"; fi

root="$(pwd)"
backup="$(mktemp -d)"
cp Cargo.toml Cargo.lock "$backup/"
# Absolute destination: the script changes directory before it exits.
trap 'cp "$backup/Cargo.toml" "$backup/Cargo.lock" "$root/" && rm -rf "$backup"' EXIT

# The only line starting with `version =` is in [workspace.package]. Cargo updates the workspace
# crates' versions in Cargo.lock by itself during the build; other dependencies stay locked.
sed -i.bak "s/^version = \".*\"/version = \"${version#v}\"/" Cargo.toml
rm Cargo.toml.bak
cargo build --release -p velo-gui -p velo-cli >&2

# The macOS binaries must run on macOS 15 (MACOSX_DEPLOYMENT_TARGET in .cargo/config.toml).
if [ "$os" = macos ]; then
    for bin in velo velo-cli; do
        minos="$(otool -l "target/release/$bin" | awk '$1 == "minos" { print $2; exit }')"
        if [ "$minos" != 15.0 ]; then
            echo "$bin needs macOS ${minos:-(unknown)}, not 15.0" >&2
            exit 1
        fi
    done
fi

name="velo-$version-$os-$arch"
rm -rf "dist/$name" "dist/$name.zip" "dist/$name.tar.gz"
mkdir -p "dist/$name"
for bin in velo velo-cli; do
    cp "target/release/$bin$exe" "dist/$name/"
done
cp README.md LICENSE "dist/$name/"

(
    cd dist
    if [ "$os" = windows ]; then
        powershell -NoProfile -Command "Compress-Archive -Path '$name' -DestinationPath '$name.zip'"
        echo "dist/$name.zip"
    else
        tar -czf "$name.tar.gz" "$name"
        echo "dist/$name.tar.gz"
    fi
)

if [ "$installers" = true ]; then
    case "$os" in
        macos) formats=dmg ;;
        windows) formats=nsis ;;
        linux) formats=deb,appimage ;;
    esac
    rm -rf dist/installers
    # AppImage tools are themselves AppImages; this runs them without FUSE (CI has none).
    APPIMAGE_EXTRACT_AND_RUN=1 cargo packager -p velo-gui --release --formats "$formats" >&2
    # cargo-packager names files after the product; give them the archive's name.
    for file in dist/installers/*; do
        case "$file" in
            *.dmg) ext=dmg ;;
            *.exe) ext=exe ;;
            *.deb) ext=deb ;;
            *.AppImage) ext=AppImage ;;
            *) continue ;;
        esac
        mv "$file" "dist/$name.$ext"
        echo "dist/$name.$ext"
    done
fi

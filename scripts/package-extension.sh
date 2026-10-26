#!/usr/bin/env bash
# Build the browser extension and zip it once per browser, for a release.
#
#   ./scripts/package-extension.sh v0.2.0
#     -> dist/velo-v0.2.0-extension-chromium.zip   (and one per browser in extension/manifests/)
#
# The extension's version is the release's without the `v` (a nightly build's `-nightly.<date>`
# is dropped: browsers take only numbers). Needs Node 22 and npm. Stdout lists the zips made.
set -euo pipefail

cd "$(dirname "$0")/.."

version="${1:?usage: $0 <version tag, e.g. v0.2.0>}"
numbers="${version#v}"
numbers="${numbers%%-*}"

root="$(pwd)"
backup="$(mktemp)"
cp extension/package.json "$backup"
# Put package.json back as it was, version and all.
trap 'cp "$backup" "$root/extension/package.json" && rm -f "$backup"' EXIT
cd extension
npm ci --no-audit --no-fund >&2
npm pkg set version="$numbers" >&2
npm run build >&2
mkdir -p "$root/dist"
for dir in dist/*/; do
    browser="$(basename "$dir")"
    zip="$root/dist/velo-$version-extension-$browser.zip"
    rm -f "$zip"
    (cd "$dir" && zip -qr "$zip" .)
    echo "dist/velo-$version-extension-$browser.zip"
done

#!/usr/bin/env bash
# Print the part of a GitHub release's notes that says which archive to download.
#
#   ./scripts/release-notes.sh v0.2.0
#
# Used by release.yml and nightly-release.yml; the archive names match package-release.sh.
set -euo pipefail

version="${1:?usage: $0 <version, e.g. v0.2.0>}"

cat <<NOTES
Download the archive for your system:
- Windows (x86_64): \`velo-$version-windows-x86_64.zip\`
- Windows (ARM64): \`velo-$version-windows-arm64.zip\`
- macOS, Apple Silicon: \`velo-$version-macos-arm64.tar.gz\`
- macOS, Intel: \`velo-$version-macos-x86_64.tar.gz\`
- Linux (x86_64): \`velo-$version-linux-x86_64.tar.gz\`

Each archive has \`velo\` (Velo Download Manager) and \`velo-cli\` (the command-line tool).
On Linux, Velo needs fontconfig and libxkbcommon installed. macOS 15 or later is required.
The builds aren't signed or notarized: on macOS, run \`xattr -dr com.apple.quarantine <unpacked folder>\` once if it's blocked; on Windows, SmartScreen may warn on first launch.
NOTES

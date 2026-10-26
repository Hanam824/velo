#!/usr/bin/env bash
# Print the part of a GitHub release's notes that says which file to download.
#
#   ./scripts/release-notes.sh v0.2.0
#
# Used by release.yml and nightly-release.yml; the file names match package-release.sh.
set -euo pipefail

version="${1:?usage: $0 <version, e.g. v0.2.0>}"

cat <<NOTES
Download the installer for your system:
- Windows (x86_64): \`velo-$version-windows-x86_64.exe\`
- Windows (ARM64): \`velo-$version-windows-arm64.exe\`
- macOS, Apple Silicon: \`velo-$version-macos-arm64.dmg\`
- macOS, Intel: \`velo-$version-macos-x86_64.dmg\`
- Linux (x86_64): \`velo-$version-linux-x86_64.deb\` or \`velo-$version-linux-x86_64.AppImage\`

Each system also has a plain archive (\`.zip\` on Windows, \`.tar.gz\` elsewhere) with \`velo\` (Velo Download Manager) and \`velo-cli\` (the command-line tool).
The Windows installer installs for the current user, with no admin rights needed. macOS 15 or later is required.
On Linux, the archive and the AppImage need fontconfig and libxkbcommon installed.
The builds aren't signed or notarized: on macOS, run \`xattr -dr com.apple.quarantine "/Applications/Velo Download Manager.app"\` once if it's blocked; on Windows, SmartScreen may warn on first launch.
NOTES

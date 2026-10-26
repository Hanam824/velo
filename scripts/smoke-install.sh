#!/usr/bin/env bash
# Install the installer that package-release.sh made for this machine, and check that the
# installed velo-cli runs and reports the expected version.
#
#   ./scripts/smoke-install.sh v0.1.0
#
# Linux installs the .deb (needs sudo), macOS mounts the .dmg, and Windows (Git Bash) runs the
# NSIS installer silently for the current user.
set -euo pipefail

cd "$(dirname "$0")/.."

version="${1:?usage: $0 <version tag, e.g. v0.1.0>}"
expected="velo-cli ${version#v}"

check() {
    local actual
    actual="$("$1" --version)"
    echo "$1: $actual"
    if [ "$actual" != "$expected" ]; then
        echo "expected \"$expected\"" >&2
        exit 1
    fi
}

case "$(uname -s)" in
    Linux)
        sudo apt-get install -y "./$(ls dist/velo-"$version"-linux-*.deb)"
        check velo-cli
        test -x /usr/bin/velo
        ;;
    Darwin)
        mount="$(mktemp -d)"
        hdiutil attach -nobrowse -readonly -mountpoint "$mount" dist/velo-"$version"-macos-*.dmg
        trap 'hdiutil detach "$mount" -quiet' EXIT
        app="$mount/Velo Download Manager.app"
        check "$app/Contents/MacOS/velo-cli"
        test -x "$app/Contents/MacOS/velo"
        ;;
    MINGW* | MSYS* | CYGWIN*)
        installer="$(ls dist/velo-"$version"-windows-*.exe)"
        # Waits for the silent installer to finish before looking for what it installed.
        powershell -NoProfile -Command "Start-Process -Wait -FilePath '$installer' -ArgumentList '/S'"
        cli="$(find "$(cygpath "$LOCALAPPDATA")" -name velo-cli.exe -path '*Velo*' | head -n 1)"
        if [ -z "$cli" ]; then
            echo "velo-cli.exe wasn't installed under %LOCALAPPDATA%" >&2
            exit 1
        fi
        check "$cli"
        test -f "$(dirname "$cli")/velo.exe"
        ;;
    *)
        echo "unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac

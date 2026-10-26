#!/usr/bin/env bash
# Build "Velo Safari Extension.app", the macOS app that carries Velo's extension into Safari,
# and zip it for a release. Needs Xcode (not just the command line tools) and Node 22.
#
#   ./scripts/build-safari.sh v0.2.0
#     -> dist/velo-v0.2.0-safari-extension-macos.zip
#
# The Xcode project is generated each time by Apple's converter from the built extension
# (extension/dist/safari), then given Velo's native handler (extension/safari/) and the sandbox
# exception it needs to reach the app's socket in /tmp. The app is signed ad hoc, not with a
# Developer ID, so Safari runs it only with Develop → Allow Unsigned Extensions. It runs on
# Apple Silicon and Intel. Stdout is the zip's path.
set -euo pipefail

cd "$(dirname "$0")/.."

version="${1:?usage: $0 <version tag, e.g. v0.2.0>}"
numbers="${version#v}"
numbers="${numbers%%-*}"
root="$(pwd)"
work="$root/target/safari"
name="Velo Safari Extension"

rm -rf "$work"
mkdir -p "$work" "$root/dist"
(cd extension && npm ci --no-audit --no-fund && npm run build -- safari) >&2

xcrun safari-web-extension-converter extension/dist/safari \
    --project-location "$work" --app-name "$name" \
    --bundle-identifier io.github.hanam824.velo.safari \
    --swift --macos-only --copy-resources --no-open --no-prompt --force >&2

project="$(find "$work" -maxdepth 4 -name '*.xcodeproj' | head -n 1 || true)"
handler="$(find "$work" -name SafariWebExtensionHandler.swift | head -n 1 || true)"
if [ -z "$project" ] || [ -z "$handler" ]; then
    echo "the converter's project isn't laid out as expected:" >&2
    find "$work" -maxdepth 4 -not -path '*/Resources/*' >&2
    exit 1
fi
cp extension/safari/SafariWebExtensionHandler.swift "$handler"
# The handler connects to the app's socket in /tmp: give the extension target (only) our
# entitlements, which add that to its sandbox.
cp extension/safari/Extension.entitlements "$(dirname "$handler")/Extension.entitlements"
python3 - "$project/project.pbxproj" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
def add(match):
    block = match.group(0)
    if "safari.Extension" not in block or "CODE_SIGN_ENTITLEMENTS" in block:
        return block
    return block.replace("buildSettings = {", 'buildSettings = {\n\t\t\t\tCODE_SIGN_ENTITLEMENTS = "Velo Safari Extension Extension/Extension.entitlements";', 1)
text, count = re.subn(r"isa = XCBuildConfiguration;.*?name = \w+;", add, text, flags=re.S)
# The converter names the app after itself; the extension's identifier must start with the app's.
text = re.sub(r"(PRODUCT_BUNDLE_IDENTIFIER = )\"?io\.github\.hanam824\.velo\.Velo-Safari-Extension\"?;", r"\1io.github.hanam824.velo.safari;", text)
if "Extension.entitlements" not in text:
    sys.exit("couldn't find the extension's build settings")
open(path, "w").write(text)
PY

xcodebuild -project "$project" -scheme "$name" -configuration Release \
    -derivedDataPath "$work/build" \
    ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO MACOSX_DEPLOYMENT_TARGET=15.0 \
    MARKETING_VERSION="$numbers" CURRENT_PROJECT_VERSION="$numbers" \
    CODE_SIGN_IDENTITY=- CODE_SIGN_STYLE=Manual DEVELOPMENT_TEAM= \
    build >&2

app="$work/build/Build/Products/Release/$name.app"
zip="$root/dist/velo-$version-safari-extension-macos.zip"
rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"
echo "dist/velo-$version-safari-extension-macos.zip"

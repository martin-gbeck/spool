#!/usr/bin/env bash
# Builds the engine and the app, and installs /Applications/Read Aloud.app.
#
#   ./build.sh          build, install, and start it
#   ./build.sh --no-open
#
# Signed with the first Apple Development identity in the keychain, so the
# Accessibility grant survives rebuilds (an ad-hoc signature changes every
# build, and macOS would forget the grant each time). SIGN_IDENTITY overrides.
# Build output stays in ~/.cache, outside anything iCloud might sync.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="/Applications/Read Aloud.app"
SWIFT_SCRATCH="$HOME/.cache/read-aloud-swift"
CLI_LINK="$HOME/.local/bin/rust-tts"

echo "==> engine"
(cd "$ROOT/engine" && cargo build --release)
ENGINE="$(cd "$ROOT/engine" && cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/tts-mcp"

echo "==> app"
swift build -c release --package-path "$ROOT/app" --scratch-path "$SWIFT_SCRATCH"
BIN="$(swift build -c release --package-path "$ROOT/app" --scratch-path "$SWIFT_SCRATCH" --show-bin-path)/ReadAloud"

echo "==> stopping the running copy"
# Builds before 2026-10-30 kept the engine in MacOS, later ones in Helpers.
for old in "$APP/Contents/Helpers/rust-tts" "$APP/Contents/MacOS/rust-tts"; do
    [[ -x "$old" ]] && "$old" quit >/dev/null 2>&1 || true
done
# Also the engine from before the app existed, which held the same socket.
if [[ -f "$CLI_LINK" && ! -L "$CLI_LINK" ]]; then
    "$CLI_LINK" quit >/dev/null 2>&1 || true
fi
# A menu bar app ignores AppleScript's quit, so this goes by process.
pkill -f "Read Aloud.app/Contents/MacOS/ReadAloud" || true
sleep 0.5

echo "==> bundle"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ReadAloud"
# Not in MacOS: an engine there counts as the app itself to macOS, so with the
# window closed, opening Read Aloud only brought the engine forward.
cp "$ENGINE" "$APP/Contents/Helpers/rust-tts"
cp "$ROOT/app/Resources/Info.plist" "$APP/Contents/Info.plist"
if [[ -f "$ROOT/app/Resources/AppIcon.icns" ]]; then
    cp "$ROOT/app/Resources/AppIcon.icns" "$APP/Contents/Resources/"
fi

echo "==> sign"
IDENTITY="${SIGN_IDENTITY:-$(security find-identity -p codesigning -v \
    | sed -nE 's/.*"(Apple Development[^"]*)".*/\1/p' | head -1)}"
if [[ -z "$IDENTITY" ]]; then
    echo "no Apple Development identity, signing ad-hoc (Accessibility resets on every build)"
    IDENTITY="-"
fi
codesign --force --sign "$IDENTITY" "$APP/Contents/Helpers/rust-tts"
codesign --force --sign "$IDENTITY" "$APP"
codesign --verify "$APP"

# The CLI and the vault's Python client both find the engine here.
mkdir -p "$(dirname "$CLI_LINK")"
rm -f "$CLI_LINK"
ln -s "$APP/Contents/Helpers/rust-tts" "$CLI_LINK"

echo "==> installed $APP"
if [[ "${1:-}" != "--no-open" ]]; then
    open "$APP"
fi

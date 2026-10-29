#!/usr/bin/env bash
# Builds the menu bar app and installs it as ~/.local/bin/rust-tts.
#
# The target directory lives outside the vault on purpose: cargo writes
# gigabytes of intermediate files and iCloud would sync every one of them.
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/rust-tts-target}"
INSTALL_PATH="$HOME/.local/bin/rust-tts"

cd "$SRC_DIR"
CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release

mkdir -p "$(dirname "$INSTALL_PATH")"
# Replace rather than overwrite: a running app holds its inode open.
rm -f "$INSTALL_PATH"
cp "$TARGET_DIR/release/tts-mcp" "$INSTALL_PATH"

echo "installed $INSTALL_PATH"
echo "restart a running app with: rust-tts quit"

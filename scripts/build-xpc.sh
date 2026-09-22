#!/usr/bin/env bash
set -euo pipefail

# Builds Phase4Engine.xpc, the unsigned XPC service bundle the Phase4 app embeds and signs.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="$ROOT/target/xpc/Phase4Engine.xpc"
PLIST="$BUNDLE/Contents/Info.plist"
VERSION="$(cargo pkgid --manifest-path "$ROOT/Cargo.toml" | sed -E 's/.*[#@]//')"

cargo build --release --bin phase4-xpc --manifest-path "$ROOT/Cargo.toml"

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS"
cp "$ROOT/resources/xpc/Info.plist" "$PLIST"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" -c "Set :CFBundleVersion $VERSION" "$PLIST"
cp "$ROOT/target/release/phase4-xpc" "$BUNDLE/Contents/MacOS/Phase4Engine"
plutil -lint "$PLIST"

echo "$BUNDLE"

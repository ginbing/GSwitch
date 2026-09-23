#!/usr/bin/env bash
set -euo pipefail
id="$1"
version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
case "$id" in
  macos-silicon) target=aarch64-apple-darwin; suffix=aarch64; arch=arm64 ;;
  macos-intel) target=x86_64-apple-darwin; suffix=x64; arch=x86_64 ;;
  *) echo "Unknown macOS target: $id" >&2; exit 1 ;;
esac
bundle="src-tauri/target/$target/release/bundle"
dmg="$bundle/dmg/GSwitch_${version}_${suffix}.dmg"
archive="$bundle/macos/GSwitch_${version}_${suffix}.app.tar.gz"
test -s "$dmg"
test -s "$archive"
mount="$RUNNER_TEMP/gswitch-release-smoke-$id"
mkdir -p "$mount"
trap 'hdiutil detach "$mount" -quiet || true' EXIT
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount" -quiet
app="$(find "$mount" -maxdepth 1 -name '*.app' -print -quit)"
test -n "$app"
actual="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
test "$actual" = "$version"
binary="$(find "$app/Contents/MacOS" -maxdepth 1 -type f -print -quit)"
test -n "$binary"
lipo -archs "$binary" | grep -qw "$arch"
codesign --verify --deep --strict "$app"
tar -tzf "$archive" | grep '\.app/Contents/Info.plist$' >/dev/null
echo "DMG mount, app version, architecture, code signature, and updater archive passed for $id"

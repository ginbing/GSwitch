#!/usr/bin/env bash
set -euo pipefail
version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
bundle=src-tauri/target/release/bundle
appimage="$bundle/appimage/GSwitch_${version}_amd64.AppImage"
deb="$bundle/deb/GSwitch_${version}_amd64.deb"
test -s "$appimage"
test -s "$deb"
test "$(dpkg-deb --field "$deb" Version)" = "$version"
test "$(dpkg-deb --field "$deb" Architecture)" = amd64
smoke="$RUNNER_TEMP/gswitch-release-smoke"
mkdir -p "$smoke/deb" "$smoke/appimage"
dpkg-deb --extract "$deb" "$smoke/deb"
find "$smoke/deb" -type f -name gswitch -print -quit | grep -q .
(
  cd "$smoke/appimage"
  "$GITHUB_WORKSPACE/$appimage" --appimage-extract >/dev/null
)
test -f "$smoke/appimage/squashfs-root/AppRun"
find "$smoke/appimage/squashfs-root" -name '*.desktop' -print -quit | grep -q .
find "$smoke/appimage/squashfs-root" -type f -name gswitch -print -quit | grep -q .
echo "Debian metadata, package payload, and AppImage extraction passed for $version"

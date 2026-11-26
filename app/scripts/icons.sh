#!/bin/sh
# Regenerate src-tauri/icons/ from app-icon.svg (#63). Run from anywhere: `npm run icons` in app/.
set -eu
cd "$(dirname "$0")/.."

npx tauri icon app-icon.svg

# The app is macOS only.
rm -rf src-tauri/icons/android src-tauri/icons/ios

# macOS 26 and later draw an icon inside a grey frame at the sizes where the .icns carries
# its own 16 and 32 px images. Without them it scales the 128 px and larger images down and
# shows the icon as drawn, as it does in the Dock.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
iconutil -c iconset src-tauri/icons/icon.icns -o "$tmp/icon.iconset"
rm "$tmp"/icon.iconset/icon_16x16*.png "$tmp"/icon.iconset/icon_32x32*.png
iconutil -c icns "$tmp/icon.iconset" -o src-tauri/icons/icon.icns

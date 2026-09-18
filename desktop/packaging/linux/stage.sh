#!/usr/bin/env bash
# Stage the Linux install tree the two non-Tauri packagers consume: the Arch
# package (pack-pacman.sh) and the flatpak (packaging/flatpak/). Tauri's own
# bundlers do not cover either — its Linux targets are deb/rpm/appimage, full
# stop — so both build from one FHS tree instead of two drifting copies of the
# install layout.
#
# The names are the ones flatpak requires: the desktop entry, AppStream
# metainfo, and the hicolor icon are looked up by APP ID, not by binary name.
#
# usage: stage.sh <binary> <outdir>
set -euo pipefail

binary=${1:?usage: stage.sh <binary> <outdir>}
out=${2:?usage: stage.sh <binary> <outdir>}

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
icons=$here/../../src-tauri/icons
app_id=app.talaria.desktop

install -Dm755 "$binary" "$out/usr/bin/talaria-desktop"
install -Dm644 "$here/$app_id.desktop" "$out/usr/share/applications/$app_id.desktop"
install -Dm644 "$here/$app_id.metainfo.xml" "$out/usr/share/metainfo/$app_id.metainfo.xml"

for size in 32 128; do
  install -Dm644 "$icons/${size}x${size}.png" \
    "$out/usr/share/icons/hicolor/${size}x${size}/apps/$app_id.png"
done
install -Dm644 "$icons/128x128@2x.png" \
  "$out/usr/share/icons/hicolor/256x256/apps/$app_id.png"
install -Dm644 "$icons/icon.png" \
  "$out/usr/share/icons/hicolor/512x512/apps/$app_id.png"
#!/usr/bin/env bash
# Repack the AppImage tauri just built, minus the libraries that have to come
# from the host.
#
# WHY. linuxdeploy's gtk plugin deploys GTK's whole dependency closure, which
# includes libwayland-client.so.0 — a library whose entire job is to talk to the
# LOCAL compositor and driver. Shipping the build distro's copy makes the bundle
# abort on any host whose Wayland/Mesa stack differs from the build image's; on
# Arch it dies before a window exists:
#
#   Could not create surfaceless EGL display: EGL_BAD_ALLOC. Aborting...
#
# The Tauri CLI offers no way to tell linuxdeploy to exclude a library — its
# AppImage config has `files` (things to add) and nothing to drop — so the
# AppDir tauri produced is trimmed here and linuxdeploy is run once more over
# it, with the exclusion in place so the second pass cannot put the library
# back.
#
# RUN IT WHERE THE BUNDLE WAS BUILT. linuxdeploy resolves dependencies against
# the host, so a pass on a different distro re-deploys that distro's libraries
# onto the bundle — in the Arch workstation's case it dies on libicudata.so.74,
# an Ubuntu soname that does not exist there. In CI that is the same Ubuntu
# runner the first pass used, which is exactly right.
#
# usage: repack-appimage.sh <bundle-dir> <version>
set -euo pipefail

bundle_dir=${1:?usage: repack-appimage.sh <bundle-dir> <version>}
version=${2:?usage: repack-appimage.sh <bundle-dir> <version>}

# The one exclusion: it is the library that made this script necessary, and a
# host without it is not a host this app can run on at all.
exclude=(libwayland-client.so.0)

appdir=$(echo "$bundle_dir"/*.AppDir)
[ -d "$appdir" ] || {
  echo "no AppDir under $bundle_dir — build the bundle first" >&2
  exit 1
}

built=$(echo "$bundle_dir"/Talaria_*_*.AppImage)
[ -f "$built" ] || {
  echo "no AppImage under $bundle_dir — build the bundle first" >&2
  exit 1
}
arch=${built%.AppImage}   # …_amd64
arch=${arch##*_}
case "$arch" in
  amd64) tools_arch=x86_64 ;;
  arm64) tools_arch=aarch64 ;;
  *)
    echo "unsupported AppImage architecture: $arch" >&2
    exit 1
    ;;
esac

tools=$(ls -t "${XDG_CACHE_HOME:-$HOME/.cache}"/tauri/linuxdeploy-*.AppImage 2>/dev/null | head -1)
[ -n "$tools" ] || {
  echo "linuxdeploy is not in tauri's tool cache (~/.cache/tauri) — run a bundle build first" >&2
  exit 1
}

for lib in "${exclude[@]}"; do
  if [ -e "$appdir/usr/lib/$lib" ]; then
    rm -f "$appdir/usr/lib/$lib"
    echo "excluded $lib from the AppDir"
  fi
done

out=$bundle_dir/Talaria_${version}_${arch}.AppImage
OUTPUT=$out \
  ARCH=$tools_arch \
  APPIMAGE_EXTRACT_AND_RUN=1 \
  "$tools" --appimage-extract-and-run \
  --appdir "$appdir" \
  --exclude-library 'libwayland-client.so*' \
  --output appimage

test -s "$out" || {
  echo "$out was not produced" >&2
  exit 1
}
# The stale first-pass bundle would otherwise ride into the release under a
# name nobody reads twice.
if [ "$built" != "$out" ]; then rm -f "$built"; fi
echo "repacked $out"
ls -l "$(dirname "$built")"
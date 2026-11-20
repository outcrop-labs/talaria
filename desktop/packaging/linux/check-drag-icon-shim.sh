#!/usr/bin/env bash
# Is the drag-image shim live in this Talaria Desktop binary?
#
#   desktop/packaging/linux/check-drag-icon-shim.sh <talaria-desktop binary>
#
# WebKitGTK hands GTK its drag image in device pixels without saying so, and
# on a display scaled above 100% the ghost came out that many times the web
# size. The shell fixes it by defining gtk_drag_set_icon_surface itself
# (desktop/src-tauri/src/main.rs and drag_icon.rs; docs/DESKTOP.md, "Drag
# images on a HiDPI Linux display"): WebKit's call lands there first, because
# the loader searches the executable before libgtk-3. That works only while
# two things hold, and either can stop holding without a sound:
#
#   1. the binary exports the symbol: it is DEFINED (not UND) in .dynsym.
#      A linker or link-flag change could leave it out (build.rs asks for it
#      by name);
#   2. the loader binds libwebkit2gtk-4.1's import of it to the binary, not to
#      libgtk-3. LD_DEBUG=bindings prints each binding; LD_BIND_NOW makes the
#      loader resolve them all before main, so this needs no display: the
#      shell then gives up for lack of one, and its exit status is not read.
#
# Exit 0 when both hold, 1 with the reason when either does not, 2 on usage.
# ci.yml's desktop job runs it on the debug build, desktop-package.yml on the
# binary the deb installed. Needs readelf (binutils) and timeout (coreutils).
set -euo pipefail

exe=${1:-}
if [ -z "$exe" ] || [ ! -x "$exe" ]; then
  echo "usage: $0 <talaria-desktop binary>" >&2
  exit 2
fi
sym=gtk_drag_set_icon_surface

# 1. Defined in the dynamic symbol table: readelf's columns are
#    Num: Value Size Type Bind Vis Ndx Name.
entry=$(readelf --dyn-syms --wide "$exe" | awk -v s="$sym" '$8 == s' || true)
echo "readelf --dyn-syms: ${entry:-no entry for $sym}"
if ! printf '%s\n' "$entry" | awk '$4 == "FUNC" && $5 == "GLOBAL" && $7 ~ /^[0-9]+$/ { ok = 1 } END { exit !ok }'; then
  echo "FAIL: $exe does not export $sym as a defined function, so WebKitGTK calls GTK's own and" \
    "the drag ghost is the scale factor times too big on a HiDPI display (see this script's header)"
  exit 1
fi

# 2. What libwebkit2gtk-4.1's import binds to.
trace=$(mktemp)
trap 'rm -f "$trace"' EXIT
env -u DISPLAY -u WAYLAND_DISPLAY LD_BIND_NOW=1 LD_DEBUG=bindings \
  timeout 30 "$exe" >/dev/null 2>"$trace" </dev/null || true
binds=$(grep -F "\`$sym'" "$trace" | grep -F 'libwebkit2gtk-4.1' || true)
echo "LD_DEBUG=bindings for $sym:"
printf '  %s\n' "${binds:-none}"
name=$(basename "$exe")
if [ -z "$binds" ]; then
  echo "FAIL: the loader bound no import of $sym from libwebkit2gtk-4.1: the WebKitGTK on this" \
    "machine does not call it, or the trace did not run (its first lines follow)"
  head -n 20 "$trace"
  exit 1
fi
if printf '%s\n' "$binds" | grep -vqE " to [^ ]*${name} \[0\]: "; then
  echo "FAIL: libwebkit2gtk-4.1 binds $sym somewhere other than $name, so the shim never runs"
  exit 1
fi
echo "PASS: $name exports $sym and libwebkit2gtk-4.1 binds to it"

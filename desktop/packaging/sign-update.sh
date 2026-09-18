#!/bin/sh
# Sign one updater payload (AppImage / .app.tar.gz / NSIS exe). No-op without
# TAURI_SIGNING_PRIVATE_KEY so a local or unsigned CI build still produces
# installers — a stable release's attach job is what refuses to ship without
# signatures (write-latest-json.py).
set -eu
file=$1
if [ ! -f "$file" ]; then
  echo "sign-update: no such file: $file" >&2
  exit 1
fi
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "sign-update: TAURI_SIGNING_PRIVATE_KEY unset — skipping $file"
  exit 0
fi
cd "$(dirname "$0")/.."
bunx tauri signer sign "$file"
echo "signed $file"

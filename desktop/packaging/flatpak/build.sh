#!/usr/bin/env bash
# Build the flatpak bundle: stage the tree, let flatpak-builder install it into
# /app against the GNOME runtime, then export a single-file .flatpak.
#
# The runtime must already be installed (CI installs org.gnome.Platform//50 and
# its SDK; check the manifest's runtime-version against what `flatpak remote-ls
# --runtime flathub` offers before bumping it).
#
# usage: build.sh <binary> <version> <outdir>
set -euo pipefail

binary=${1:?usage: build.sh <binary> <version> <outdir>}
version=${2:?usage: build.sh <binary> <version> <outdir>}
out=${3:?usage: build.sh <binary> <version> <outdir>}

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
app_id=app.talaria.desktop
arch=$(uname -m)

for tool in flatpak flatpak-builder; do
  command -v "$tool" >/dev/null || {
    echo "$tool is not installed — see docs/DESKTOP.md (installers)" >&2
    exit 1
  }
done

# staging has to sit beside the manifest (a flatpak source path is relative to
# it); everything else the build produces is scratch and stays out of the tree.
staging=$here/staging
work=$(mktemp -d)
trap 'rm -rf "$staging" "$work"' EXIT

rm -rf "$staging"
"$here/../linux/stage.sh" "$binary" "$staging"

mkdir -p "$out"
bundle=$out/Talaria-$version-$arch.flatpak

# --disable-rofiles-fuse: rofiles-fuse needs /dev/fuse and a real mount
# namespace, which CI sandboxes routinely deny. The flag trades a little build
# speed for not failing on the environment.
flatpak-builder \
  --disable-rofiles-fuse \
  --force-clean \
  --state-dir="$work/state" \
  --repo="$work/repo" \
  "$work/build" \
  "$here/$app_id.yml"

# --runtime-repo: the bundle records where its runtime comes from, so
# `flatpak install ./Talaria-*.flatpak` knows to fetch GNOME 50 from flathub
# instead of failing on a missing runtime.
flatpak build-bundle "$work/repo" "$bundle" "$app_id" \
  --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
echo "wrote $bundle"
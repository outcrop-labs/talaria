#!/usr/bin/env bash
# Turn the staged FHS tree into an Arch package. `makepkg` is not involved:
# there is nothing to compile, so the package is the tree plus a .PKGINFO,
# tarred in that order (a package's first member is its metadata).
#
# The package is named -bin because that is what the suffix means in Arch: it
# ships a prebuilt binary rather than a build recipe. A .MTREE is optional and
# omitted; pacman installs a package without one.
#
# usage: pack-pacman.sh <staged-tree> <version> <out.pkg.tar.zst>
set -euo pipefail

tree=${1:?usage: pack-pacman.sh <staged-tree> <version> <out.pkg.tar.zst>}
version=${2:?usage: pack-pacman.sh <staged-tree> <version> <out.pkg.tar.zst>}
out=${3:?usage: pack-pacman.sh <staged-tree> <version> <out.pkg.tar.zst>}

# pacman's version grammar is pkgver-pkgrel, and pkgver may not contain another
# '-': the release tag's `0.2.0-rc.1` is not a legal pkgver (`0.2.0-rc.1-1`
# loads as an invalid package), while `0.2.0_rc.1-1` does. pkgrel 1 = the first
# build of that upstream version.
pkgname=talaria-desktop-bin
pkgver=${version//-/_}-1
arch=$(uname -m) # x86_64 | aarch64 — Arch's own spelling of both

cat >"$tree/.PKGINFO" <<EOF
pkgname = $pkgname
pkgbase = $pkgname
xdata = pkgtype=pkg
pkgver = $pkgver
pkgdesc = Official desktop client for Talaria
url = https://talariaworks.ai
builddate = $(date -u +%s)
packager = Outcrop Labs LLC
size = $(du -sb "$tree/usr" | cut -f1)
arch = $arch
license = MIT
depend = webkit2gtk-4.1
depend = gtk3
EOF

tar --zstd -cf "$out" -C "$tree" .PKGINFO usr
echo "wrote $out"
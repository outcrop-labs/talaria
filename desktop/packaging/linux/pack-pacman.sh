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
# loads as an invalid package). It must also sort below `0.2.0`, or the
# release will not install over its RC: pacman's vercmp has no `~`, and reads
# `0.2.0_rc.1` (or `0.2.0~rc.1`) as newer than `0.2.0`, but lets a trailing
# segment that starts with a letter lose to the end of the string. So the
# pre-release separator is dropped — `0.2.0rc.1`, Arch's own spelling, with
# rc.1 < rc.2 < rc.10 still — and any later '-' becomes '.'. The versions
# desktop-package.yml stamps are X.Y.Z, X.Y.Z-rc.N and 0.0.0-sha<hex>, each
# pre-release starting with a letter. pkgrel 1 = the first build of that
# upstream version.
pkgname=talaria-desktop-bin
pre=${version/-/}
pkgver=${pre//-/.}-1
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
optdepend = libayatana-appindicator: the tray icon (Close keeps Talaria in the tray; without it Close minimizes)
EOF

tar --zstd -cf "$out" -C "$tree" .PKGINFO usr
echo "wrote $out"
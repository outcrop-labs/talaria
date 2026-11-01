#!/usr/bin/env bash
# The deb and the rpm RECOMMEND the tray library instead of requiring it.
#
#   recommend-tray.sh <talaria.deb> <talaria.rpm>     (both rewritten in place)
#
# WHY. With tauri's `tray-icon` feature, the pinned @tauri-apps/cli adds the
# appindicator library to the deb's Depends (`libayatana-appindicator3-1`)
# and the rpm's Requires (`libayatana-appindicator3.so.1()(64bit)`), and no
# config removes them (tauri-cli's interface/rust.rs pushes them after reading
# `bundle.linux.*.depends`). But the shell does not link the library: it opens
# it with dlopen, and runs without it — Close then minimizes instead of hiding
# to a tray nobody can see (src-tauri/src/background.rs). A hard dependency
# only keeps Talaria off a system without it. tauri.linux.conf.json already
# lists the library under `recommends` for both, so the bundler writes the
# Recommends line; this takes it back out of Depends and Requires.
#
# The deb: unpacked with dpkg-deb, the control file edited, packed again —
# the same files, the same md5sums. The rpm: its header cannot be edited in
# place (the signature header covers it), so it is rebuilt with rpmbuild from
# its own payload and its own header's fields, with AutoReqProv off so the
# requirement set is exactly the old one minus the library. A pre-release
# Version (`0.0.0-sha…`, `0.2.0-rc.1`) carries a `-`, which tauri's rpm crate
# writes and rpmbuild refuses; it becomes `_`, and since rpm's version compare
# treats every non-alphanumeric character as the same separator, the new
# Version orders exactly as the old one did. Both packages are then checked:
# the library is gone from the hard dependencies and present as a
# recommendation, and the rpm carries the same files as before.
set -euo pipefail

deb=$(realpath "$1")
rpm=$(realpath "$2")
lib_deb=libayatana-appindicator3-1
lib_rpm='libayatana-appindicator3.so.1()(64bit)'
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# ── the deb ───────────────────────────────────────────────────────────────────
dpkg-deb -R "$deb" "$work/deb"
python3 - "$work/deb/DEBIAN/control" "$lib_deb" <<'PY'
import re, sys
path, lib = sys.argv[1], sys.argv[2]
lines = open(path).read().rstrip('\n').split('\n')
out, recommends = [], None
for line in lines:
    if line.startswith('Depends:'):
        deps = [d.strip() for d in line[len('Depends:'):].split(',') if d.strip()]
        deps = [d for d in deps if re.split(r'[\s(]', d, maxsplit=1)[0] != lib]
        if deps:
            out.append('Depends: ' + ', '.join(deps))
    elif line.startswith('Recommends:'):
        recommends = [d.strip() for d in line[len('Recommends:'):].split(',') if d.strip()]
    else:
        out.append(line)
recommends = recommends or []
if lib not in [re.split(r'[\s(]', d, maxsplit=1)[0] for d in recommends]:
    recommends.append(lib)
# Recommends goes right after Depends, as dpkg writes it.
at = next((i + 1 for i, l in enumerate(out) if l.startswith('Depends:')), len(out))
out.insert(at, 'Recommends: ' + ', '.join(recommends))
open(path, 'w').write('\n'.join(out) + '\n')
PY
dpkg-deb --root-owner-group -Zxz -b "$work/deb" "$work/new.deb" >/dev/null
mv "$work/new.deb" "$deb"

# ── the rpm ───────────────────────────────────────────────────────────────────
q() { rpm -qp --qf "$1" "$rpm"; }
if [ -n "$(rpm -qp --scripts "$rpm")" ]; then
  echo "::error::$rpm has scriptlets this rebuild does not carry" >&2
  exit 1
fi
name=$(q '%{NAME}')
arch=$(q '%{ARCH}')
top="$work/rpm"
mkdir -p "$top/root" "$top"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
(cd "$top/root" && rpm2cpio "$rpm" | cpio -idm --quiet)
python3 - "$rpm" "$lib_rpm" "$top/root" "$top/SPECS/$name.spec" <<'PY'
import subprocess, sys
rpm, lib, root, spec = sys.argv[1:5]
def q(fmt):
    return subprocess.run(['rpm', '-qp', '--qf', fmt, rpm], check=True, capture_output=True, text=True).stdout
def lines(*flag):
    out = subprocess.run(['rpm', '-qp', *flag, rpm], check=True, capture_output=True, text=True).stdout
    return [l.strip() for l in out.splitlines() if l.strip()]
name = q('%{NAME}')
requires = [r for r in lines('--requires') if not r.startswith('rpmlib(') and r != lib]
recommends = [r for r in lines('--recommends') if r != lib] + [lib]
# rpmbuild writes the package's own provides; carry only the others.
provides = [p for p in lines('--provides') if not p.split()[0].startswith(name)]
files = []
for row in q('[%{FILEMODES}\t%{FILEUSERNAME}\t%{FILEGROUPNAME}\t%{FILENAMES}\n]').splitlines():
    mode, user, group, path = row.split('\t', 3)
    mode = int(mode)
    prefix = '%dir ' if mode & 0o170000 == 0o040000 else ''
    files.append(f'{prefix}%attr({mode & 0o7777:o},{user},{group}) "{path}"')
def field(tag, fallback=''):
    v = q('%{' + tag + '}')
    return fallback if v == '(none)' else v
# rpmbuild refuses a '-' in Version or Release; rpmvercmp reads '-' and '_'
# as the same separator, so the order is unchanged (the header comment).
def evr(tag):
    v = field(tag)
    if '-' in v:
        print(f'rpm {tag.lower()}: {v} is written {v.replace("-", "_")} (rpmbuild refuses "-"; same order)', file=sys.stderr)
    return v.replace('-', '_')
body = [
    '%define _build_id_links none',
    '%define __os_install_post %{nil}',
    '%define __strip /bin/true',
    '%define debug_package %{nil}',
    f'Name: {name}',
    f'Version: {evr("VERSION")}',
    f'Release: {evr("RELEASE")}',
    f'Summary: {field("SUMMARY", name)}',
    f'License: {field("LICENSE", "unknown")}',
    *( [f'URL: {field("URL")}'] if field("URL") else [] ),
    *( [f'Vendor: {field("VENDOR")}'] if field("VENDOR") else [] ),
    *( [f'Packager: {field("PACKAGER")}'] if field("PACKAGER") else [] ),
    f'BuildArch: {field("ARCH")}',
    'AutoReqProv: no',
    *[f'Requires: {r}' for r in requires],
    *[f'Recommends: {r}' for r in recommends],
    *[f'Provides: {p}' for p in provides],
    '',
    '%description',
    field('DESCRIPTION', name),
    '',
    '%install',
    f'cp -a "{root}/." "%{{buildroot}}/"',
    '',
    '%files',
    *files,
    '',
]
open(spec, 'w').write('\n'.join(body))
PY
rpmbuild --quiet --define "_topdir $top" -bb "$top/SPECS/$name.spec" >/dev/null
built=$(find "$top/RPMS" -name '*.rpm' | head -1)
[ -n "$built" ] || { echo "::error::rpmbuild made no package" >&2; exit 1; }

# ── checks ────────────────────────────────────────────────────────────────────
deps=$(dpkg-deb -f "$deb" Depends)
recs=$(dpkg-deb -f "$deb" Recommends)
echo "deb Depends: $deps"
echo "deb Recommends: $recs"
case "$deps" in *"$lib_deb"*) echo "::error::the deb still depends on $lib_deb" >&2; exit 1 ;; esac
case "$recs" in *"$lib_deb"*) ;; *) echo "::error::the deb does not recommend $lib_deb" >&2; exit 1 ;; esac
if rpm -qpR "$built" | grep -qF "$lib_rpm"; then
  echo "::error::the rpm still requires $lib_rpm" >&2; exit 1
fi
rpm -qp --recommends "$built" | grep -qF "$lib_rpm" || { echo "::error::the rpm does not recommend $lib_rpm" >&2; exit 1; }
if ! diff <(rpm -qp --qf '[%{FILENAMES} %{FILESIZES} %{FILEMODES}\n]' "$rpm" | sort) \
          <(rpm -qp --qf '[%{FILENAMES} %{FILESIZES} %{FILEMODES}\n]' "$built" | sort) >/dev/null; then
  echo "::error::the rebuilt rpm's files differ from the original's" >&2; exit 1
fi
echo "rpm Requires: $(rpm -qpR "$built" | grep -v '^rpmlib(' | paste -sd, -)"
echo "rpm Recommends: $(rpm -qp --recommends "$built" | paste -sd, -)"
mv "$built" "$rpm"
echo "the tray library is recommended, not required: $(basename "$deb"), $(basename "$rpm") ($arch)"

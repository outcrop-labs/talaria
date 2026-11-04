#!/usr/bin/env bash
# The deb and the rpm RECOMMEND the tray library instead of requiring it, and
# a pre-release's Version sorts below its release.
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
# requirement set is exactly the old one minus the library.
#
# THE VERSION. Tauri writes the stamped version verbatim into both, and a
# pre-release's (`0.2.0-rc.1`, a trunk `0.0.0-sha…`) sorts ABOVE its release
# there: dpkg reads `0.2.0-rc.1` as upstream 0.2.0 with Debian revision rc.1,
# newer than a bare 0.2.0, and rpm compares `0.2.0-rc.1` (which rpmbuild
# refuses anyway) or `0.2.0_rc.1` segment by segment, where the extra `rc`
# makes it newer too. So the stable release would not install over its RC —
# a downgrade. Both formats sort `~` below everything, the end of the string
# included, so the pre-release `-` becomes `~` (`0.2.0~rc.1` < `0.2.0`, and
# rc.1 < rc.2 < rc.10 still); any later `-` becomes `.`. The file names keep
# the tag's spelling. Both packages are then checked: the library is gone
# from the hard dependencies and present as a recommendation, the Version
# carries no `-` and a pre-release sorts below its release, and the rpm
# carries the same files as before.
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
    if line.startswith('Version:'):
        # The pre-release `-` is `~`, so an RC sorts below its release (the
        # header comment); a stable version has none and is left as it is.
        version = line[len('Version:'):].strip()
        spelled = version.replace('-', '~', 1).replace('-', '.')
        if spelled != version:
            print(f'deb version: {version} is written {spelled} (a pre-release sorts below its release)', file=sys.stderr)
        out.append('Version: ' + spelled)
    elif line.startswith('Depends:'):
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
# Ubuntu's rpm2cpio streams the whole of tauri's gzip payload and then exits
# 1; an extraction that came up short fails the file comparison below.
(cd "$top/root" && { rpm2cpio "$rpm" || echo "rpm2cpio exited $? (the file comparison below checks the extraction)" >&2; } | cpio -idm --quiet)
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
# rpmbuild refuses a '-' in Version or Release. Version's pre-release '-' is
# '~', so an RC sorts below its release (the header comment). Release is
# tauri's "1"; were it ever to carry a '-', '_' orders it as '-' would.
def version():
    v = field('VERSION')
    spelled = v.replace('-', '~', 1).replace('-', '.')
    if spelled != v:
        print(f'rpm version: {v} is written {spelled} (a pre-release sorts below its release)', file=sys.stderr)
    return spelled
def release():
    return field('RELEASE').replace('-', '_')
body = [
    '%define _build_id_links none',
    '%define __os_install_post %{nil}',
    '%define __strip /bin/true',
    '%define debug_package %{nil}',
    f'Name: {name}',
    f'Version: {version()}',
    f'Release: {release()}',
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
# The Version: no '-' left (dpkg would read one as a Debian revision), and a
# pre-release sorts below the release it precedes, by each format's own
# compare — the order apt and dnf use when the release is installed over it.
deb_version=$(dpkg-deb -f "$deb" Version)
rpm_version=$(rpm -qp --qf '%{VERSION}' "$built")
echo "deb Version: $deb_version"
echo "rpm Version: $rpm_version"
case "$deb_version$rpm_version" in *-*) echo "::error::a package Version still carries a '-'" >&2; exit 1 ;; esac
if [ "${deb_version%%"~"*}" != "$deb_version" ]; then
  dpkg --compare-versions "$deb_version" lt "${deb_version%%"~"*}" \
    || { echo "::error::the deb's $deb_version does not sort below ${deb_version%%"~"*}" >&2; exit 1; }
fi
if [ "${rpm_version%%"~"*}" != "$rpm_version" ]; then
  order=$(rpm --eval "%{lua: print(rpm.vercmp('$rpm_version', '${rpm_version%%"~"*}'))}")
  [ "$order" = "-1" ] \
    || { echo "::error::the rpm's $rpm_version does not sort below ${rpm_version%%"~"*} (vercmp $order)" >&2; exit 1; }
fi
if rpm -qpR "$built" | grep -qF "$lib_rpm"; then
  echo "::error::the rpm still requires $lib_rpm" >&2; exit 1
fi
rpm -qp --recommends "$built" | grep -qF "$lib_rpm" || { echo "::error::the rpm does not recommend $lib_rpm" >&2; exit 1; }
# The same files: every path with its mode, and each regular file's size and
# digest. A directory's size is whatever its builder recorded (tauri's rpm
# crate and rpmbuild differ), so it is not compared.
python3 - "$rpm" "$built" <<'PY2'
import subprocess, sys
# `--dump` reads the file list through rpm's own iterator: a tag query of
# FILESIZES answers "(none)" on Ubuntu's rpm for a package that carries
# LONGFILESIZES, as tauri's rpm crate writes them.
def files(rpm):
    out = subprocess.run(['rpm', '-qp', '--dump', rpm], check=True, capture_output=True, text=True).stdout
    rows = {}
    for line in out.splitlines():
        # path size mtime digest mode owner group isconfig isdoc rdev symlink
        parts = line.split(' ')
        if len(parts) < 11:
            continue
        name = ' '.join(parts[:-10])
        size, digest, mode = parts[-10], parts[-8], int(parts[-7], 8)
        regular = mode & 0o170000 == 0o100000
        rows[name] = (oct(mode), int(size) if regular else None, digest if regular else None)
    return rows
old, new = files(sys.argv[1]), files(sys.argv[2])
diffs = [n for n in sorted(set(old) | set(new)) if old.get(n) != new.get(n)]
for n in diffs[:20]:
    print(f'  {n}: was {old.get(n)}, now {new.get(n)}', file=sys.stderr)
if diffs:
    print(f"::error::the rebuilt rpm's files differ from the original's ({len(diffs)} of {len(old)})", file=sys.stderr)
    sys.exit(1)
print(f'rpm files: the same {len(old)} paths, modes, sizes and digests')
PY2
echo "rpm Requires: $(rpm -qpR "$built" | grep -v '^rpmlib(' | paste -sd, -)"
echo "rpm Recommends: $(rpm -qp --recommends "$built" | paste -sd, -)"
mv "$built" "$rpm"
echo "the tray library is recommended, not required: $(basename "$deb"), $(basename "$rpm") ($arch)"

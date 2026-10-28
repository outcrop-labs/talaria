#!/bin/sh
# RUSTC_WRAPPER, pointed at by the root mise.toml.
#
# Cargo calls a wrapper as `<wrapper> <path-to-rustc> <args...>`, including
# for the `rustc -vV` probe it runs before anything else. So a wrapper that
# does not exist does not degrade — it fails cargo outright, at startup,
# before a single crate compiles:
#
#     error: could not execute process `sccache .../rustc -vV` (never executed)
#     Caused by: No such file or directory (os error 2)
#
# That is what pointing RUSTC_WRAPPER straight at `sccache` did. The
# reasoning behind it was that mise hands out the tool and the env together,
# so they arrive as a pair — they do not. mise applies `[env]` from the
# config file the moment you are in the directory, while the binary only
# exists once `mise install` has run. Every checkout between those two
# moments had a broken cargo, and `talaria dev` could not start its api.
#
# Hence this shim: use sccache when it is actually there, and plain rustc
# when it is not. The cache is an optimisation, and an optimisation must
# never be the reason a build cannot start.
#
# THE MISE SHIM IS NOT THE BINARY. Where mise runs in shims mode — a
# non-interactive shell, an IDE, an agent harness, anything whose PATH came
# from outside a mise-activated directory — the `sccache` on PATH is
# ~/.local/share/mise/shims/sccache, and a shim picks its version from the
# CURRENT DIRECTORY's mise config. Cargo runs rustc for a registry crate from
# that crate's own source dir under ~/.cargo/registry/src, where nothing pins
# sccache, so the shim answered
#
#     mise ERROR No version is set for shim: sccache
#
# and, because this line was an `exec`, the build died with it — intermittent,
# because PATH-mode activation puts the real install dir first and works.
# The workbench learned this already (prepare_env sets the wrapper to
# `mise which sccache`, api/crates/talaria-workbench-mcp/src/devenv.rs). So
# resolve the REAL binary, in this order, and never exec a shim:
#
#   1. a non-shim sccache on PATH — free, and what PATH-mode activation gives;
#   2. `mise which sccache`, asked from the REPO ROOT, where mise.toml pins it
#      (one ~20 ms spawn per rustc, and only in shims mode);
#   3. plain rustc.

is_shim() {
  case "$1" in
    */mise/shims/*) return 0 ;;
  esac
  # A shim is a symlink to the mise binary itself, wherever MISE_DATA_DIR
  # put the shims directory.
  if [ -L "$1" ]; then
    case "$(readlink -- "$1" 2>/dev/null)" in
      mise | */mise) return 0 ;;
    esac
  fi
  return 1
}

sc=
set -f
saved_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  if [ -f "$dir/sccache" ] && [ -x "$dir/sccache" ] && ! is_shim "$dir/sccache"; then
    sc=$dir/sccache
    break
  fi
done
IFS=$saved_ifs
set +f

if [ -z "$sc" ] && command -v mise >/dev/null 2>&1; then
  root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd)
  if [ -n "$root" ]; then
    sc=$(cd "$root" && mise which sccache 2>/dev/null) || sc=
  fi
  if [ -n "$sc" ] && is_shim "$sc"; then sc=; fi
fi

if [ -n "$sc" ] && [ -x "$sc" ]; then
  exec "$sc" "$@"
fi
exec "$@"

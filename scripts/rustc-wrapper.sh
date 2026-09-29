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
if command -v sccache >/dev/null 2>&1; then
  exec sccache "$@"
fi
exec "$@"

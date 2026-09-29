- **A missing sccache no longer stops cargo dead.** Pointing `RUSTC_WRAPPER`
  straight at `sccache` in the root `mise.toml` broke every cargo invocation
  on any checkout where `mise install` had not yet run: cargo calls the
  wrapper for its `rustc -vV` probe before it compiles anything, so a wrapper
  that is not on PATH is not a missed optimisation but a hard failure at
  startup — `could not execute process \`sccache ... rustc -vV\` (never
  executed)`, cargo exit 101, and `bun talaria dev` reporting "rust api
  exited 101 — the app has no api until it is restarted." The reasoning that
  shipped it was that mise hands out the tool and the env together so they
  arrive as a pair; they do not. mise applies `[env]` the moment you are in
  the directory, while the binary only exists after `mise install`, and every
  checkout between those two moments had a cargo that could not start.
  `RUSTC_WRAPPER` now points at `scripts/rustc-wrapper.sh`, which execs
  sccache when it is installed and plain rustc when it is not, so the cache
  is an optimisation again rather than a precondition. Verified on a checkout
  with no sccache installed: `bun talaria dev` failed at the probe with exit
  101 before the change and compiles normally after it, same machine, same
  command, only the wrapper different.

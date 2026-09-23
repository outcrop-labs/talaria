- **One internal-error trap: ~860 hand-written log-and-500 sites collapse onto
  `talaria_error::internal`.** The `tracing::error!("…: {e}"); return
  thrown_internal_error();` pair was written out by hand at every call site that
  did not catch an engine error, in five spellings (terminal `return`, tail
  expression, `Err(…)`, `Some(…)`, fully-qualified). Because each site re-made
  the one decision in it, about a quarter re-made it as "no log at all" — a 500
  with nothing in the log and no way to tell which read failed. `internal`
  (api/crates/talaria-error/src/lib.rs) is that pair once: it logs
  `"{context}: {e}"` and returns the same byte-exact `thrown_internal_error()`
  response. Rust `tracing::error!` in api/crates: 993 → 172; `return
  thrown_internal_error();` → 1 (inside `internal`). Two new invariant rules
  (`rust-trap-block`, `rust-thrown-internal-error-outside-its-envelope`) fail the
  next copy, and the checker now scans `api/crates` as a second source tree
  (`scripts/check-invariants.mjs`, `lang: 'rust'`; a duplicate-function-body
  detector with a name+path allow list rides along). Non-conforming sites — a
  log line quoting a value rather than the error, or an `.is_err()` that logged
  nothing — were converted by hand; those log lines gain a `: <detail>` suffix.
  Wire bytes, status codes and the route table are unchanged. Verified:
  `bun run api:check` (fmt + clippy `-D warnings` + `cargo test --workspace`,
  exit 0), `bun run check` (gen-docs `--check`: 245 routes, 25 files, no drift).

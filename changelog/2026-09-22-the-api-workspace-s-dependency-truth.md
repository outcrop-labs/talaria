- **The api workspace's dependency truth lives in one table.** 215 manifests
  each pinned their own versions — serde 168 times, sqlx 144, tokio in 21
  different feature combinations — so every version bump edited dozens of
  files and a feature added in one crate silently didn't exist in its
  siblings. `api/Cargo.toml` now carries `[workspace.dependencies]` (external
  deps with their rationale comments, then every crate as a path dep) and
  `[workspace.package]`; every manifest says only `{ workspace = true }`, and
  the 214-line members list collapsed to `members = ["crates/*"]`. The union
  entries declare exactly what cargo already unified across the build graph:
  axum's defaults are spelled out explicitly plus multipart, chrono carries
  clock for the wall-clock google/brief surfaces while the scheduler's
  epoch-ms rule stands. `scripts/codemod-workspace-deps.mjs` (stdlib-only,
  idempotent) did the rewrite and is the rebase-repair tool for any session
  that conflicts on a manifest.
  Verified: `api/Cargo.lock` byte-identical after the rewrite (the primary
  gate); `cargo metadata` package list identical (215/215); enabled feature
  sets for tokio/sqlx/axum/redis/reqwest/serde_json/chrono identical
  before/after via `cargo tree -e features -i`; `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` (479 crates) and
  `cargo test` green from `api/`; `bun run check` green; a second codemod
  run is a no-op (root detected-and-skipped, 0/214 crates touched).

- **W12d — one CI runtime setup.** `.github/actions/setup-runtime/action.yml`
  (composite) owns the `bun 1.4.0` / `node 22.x` / `rust 1.97.1` pins and the
  checkout+bun+node+install quartets; `grep` now finds those versions nowhere else
  in `.github/`. Two traps handled: every `surface` line in `ci.yml` gained
  `.github/actions/**` (a change there otherwise runs only the invariants job),
  and **checkout deliberately stays per-job** — a same-repo composite action is
  resolved from the workspace, so a step inside it can never be the first
  checkout. `migrations.yml`'s stale `actions/checkout@v4` came up to v5 (all 19
  occurrences across `.github/` are v5 now), and every touched workflow still
  parses.

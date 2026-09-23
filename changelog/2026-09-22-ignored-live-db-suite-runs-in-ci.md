- **The ignored live-database api suite runs in CI.** 31 of the 33
  integration binaries in `api/tests/` carried `#[ignore = "needs a live dev
  database"]` and were last executed by whoever remembered to — the tests that
  prove the runtime sqlx queries, the `$N::` bind casts and the store engines
  against the real schema never ran on a machine, so that entire class of
  regression surfaced as production 500s of exactly the shape `repo-traps`
  catalogues. `api-integration.yml` (its own workflow, like migrations.yml —
  ci.yml's service-free contract stays intact) boots scratch postgres:16 and
  redis:7 containers, bootstraps the schema by replaying the full MIGRATIONS
  array, then runs every integration binary with `--ignored` plus the
  workspace-wide sweep of ignored in-crate unit tests. The binary list is
  self-maintaining (a new `tests/<file>.rs` is picked up by the loop); the
  only committed knowledge is the three things that cannot run on containers
  — `typed_binds` (dev qdrant+TEI), `update_live` (real ghcr.io), and one
  `llm_models` test needing a minted `tlk_` key — each saying why, at the
  exclusion. Rule 9 in RUST-MIGRATION.md updated to match.
  Verified: YAML parses and every claim in the workflow checked against the
  tree (33 binaries listed; `minted_key_lists_the_catalog` at
  api/tests/llm_models.rs:123; the ignored in-crate unit test in
  talaria-runs-store/src/lib.rs); paths filter mirrors migrations.yml's
  shape; the suite itself runs for the first time in this PR's Actions run —
  that run is the exercise.

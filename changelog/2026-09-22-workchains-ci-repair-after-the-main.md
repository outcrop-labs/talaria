- **Workchains: CI repair after the main merge.** The merged tree failed
  the api and migrations CI jobs. Fixed: regenerated
  `ui/src/server/db/schema.snapshot.sql` (the merge hand-carried a stale
  snapshot — table ordering and `"position"` quoting drifted from what
  pg_dump emits); dropped the dead `read_gate` helper and an unused
  parameter that tripped clippy `-D warnings`; repaired
  `tests/workchains_live.rs`, which never compiled (missing `pg`
  bindings, `as_deref` on tuple options, moved `String`s); and updated
  the two unit tests that pinned the pre-workchain worlds (the views
  enum without `workchains`, the digest derivation without the
  `workchain_turn`/`workchain_paused` classes). Verified: `bun run
  api:check` green (fmt, clippy `-D warnings`, 2067 tests), two-pass
  `migrations:check` against a scratch postgres 16 (`applied: 0` on the
  second pass, snapshot matches), and full `bun run verify` green
  (svelte-check 0 errors, 1200 tests).

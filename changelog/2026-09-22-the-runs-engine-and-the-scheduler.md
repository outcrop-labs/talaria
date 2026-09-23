- **The runs engine and the scheduler cross — the port's flip.** The
  durable-run core (Redis leases with CAS renew/release, every `runs` write
  a CAS on `(id, lease_owner, state)`, the step-budget lease TTL, the
  `awaiting` guard that parks rather than fails), the six run kinds, the
  boards family with its dispatch re-entry, the SSE streams, and the whole
  registered job table — comms-decay, outreach-sweep, price-refresh,
  daily-digest, approval-escalation, notification-mail, run-reclaim,
  daily-brief, mcp-library-refresh, update-check, with `maybeRewriteBlurbs`
  a REAL job at last (dark in every proxied environment until now — its
  only TS trigger was the `/api/models` handler the proxy shadows).
  `TALARIA_SCHEDULER=rust` hands the whole schedule over in one slice —
  stop-TS, arm-Rust, never both — and the arm refuses to fire until the
  census's run kinds all have definitions. Schema ownership handed to sqlx
  with it: the same `schema_migrations` discipline (per-statement sha256
  bookkeeping, the same advisory lock, growth-only re-arm) now lives in
  `api/src/db.rs`, and the TS array is frozen. `update-check`'s apply half
  is a deliberate hold — it rebuilds `ui/dist` and restarts the bun process
  it runs in, choreography that cannot restart the Rust binary — so its
  auto half returns at cutover.

- **The README states the deployment topology.** One app instance per
  deployment; scale the box, not the instance count. The section says what
  is already multi-instance-safe (the Redis-leased scheduler, deterministic
  dispatch ids, per-step run leases), what is not (the TS ui plane;
  process-local queues such as the notify mail outbox), what
  multi-instance would need, and points at `docs/CONTAINER.md` for the
  per-host isolation case. Verified against source before landing: the
  four `per_instance` jobs that take no lease (`talaria-notify`,
  `talaria-fleet-resources`, `talaria-mcp-library`, `talaria-mcp` pkg
  reconcile), the `session_run_id` claim walk in `talaria-work-dispatch`,
  the lease-then-reclaim loop in `talaria-runs-lease`, and the three TS
  residents in `ui/src/server/rust-proxy.ts`. `bun run check` passes —
  docs link check included.

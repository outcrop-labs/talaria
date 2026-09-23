- **The effort picker now appears on deployments upgraded past its ship.** The
  stored per-model catalog's only production writer is the model-adder modal,
  so a catalog written before the effort extraction had no levels for anyone
  and the chip stayed hidden until an admin re-opened the modal. An empty read
  on `/api/models/efforts` now runs a one-time backfill — the serving
  endpoints' catalogs are refreshed live (once per endpoint; a catalog written
  by the current build never re-triggers, and a failed refresh retries no more
  than every five minutes) — and the route answers from the fresh store. Also:
  the agent chip is gone from the chat composer rails (Comms, Plan, Research) —
  the sidebar owns the conversation partner, and the rail's right side is now
  tier, effort, then the send/stop tile.

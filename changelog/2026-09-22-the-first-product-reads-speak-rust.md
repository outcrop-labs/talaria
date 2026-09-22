- **The first product reads speak Rust**: `/api/agents` (the fleet list with
  tiers and per-agent access), `/api/apps` (the enabled-app manifest),
  `/api/activity` (the merged feed with the admins-only audit kind), and
  `/api/cost` (the full ledger overview — priced windows, per-model,
  per-agent, per-day) all serve from the Rust api, byte-diffed against TS on
  the same sessions for admin, member, and anonymous callers. The proxy gains
  an exact-match list alongside its prefixes: `/api/agents` and `/api/apps`
  are whole-path migrations because their sub-routes (`register`,
  `heartbeat`, the app-server gateway) stay TS until their batches.

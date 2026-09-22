- **The fleet plane speaks Rust — every remaining `/api/fleet` route
  plus the agents register/heartbeat pair.** Sixteen route files
  crossed as one unit: the ops overview (`/api/fleet` — heartbeats
  seeded from defs, container reality filling the liveness gap so the
  online count can't disagree with the roster dots), containers, the
  defs listing, crons fleet-wide and per-agent with the per-job trio
  (delete/edit/pause-resume-run), the per-agent secrets vault door
  (PUT set, DELETE with its query-param fallback when the body won't
  parse), the seven-verb control union (up/stop/restart/roll/retire/
  unretire/delete with the audit-before-act discipline and the catch
  arm that flattens every interior failure into the generic 500),
  render, endpoints CRUD (create/patch/delete/available), federate,
  and reconcile — plus `/api/agents/register` and
  `/api/agents/{id}/heartbeat`, the shared-key fleet plane, now
  reading `TALARIA_AGENT_KEY` in the Rust process too. Two wire rules
  the byte-diff pinned down and now hold crate-wide: pg `numeric`
  columns ride the TS wire as STRINGS (postgres.js passes them through
  unparsed — a stored 3 is `"3"` — so the endpoints listing selects
  `::text`), and timestamptz-to-ms TRUNCATES where a pg cast would
  round (JS `Date` drops the fractional millisecond — the overview's
  last-used is `trunc(extract(epoch …)·1000)::bigint`). The heartbeat's
  non-uuid path 500s on both sides for different reasons that meet in
  the same envelope (postgres.js's throw vs the `$1::uuid` cast) —
  byte-verified. Verified by a 64-case byte-diff against TS (every GET
  × admin/member/anon gates, the zod 400 tables, the render POST, the
  endpoints CRUD round-trip on per-side fixtures, `available` against
  an unreachable baseUrl, fake-id 404s, and the register→heartbeat
  round-trip with the real org key): 64 byte-identical, 0 normalized,
  0 unexplained; lifecycle verbs on real agents stayed SKIP-listed
  (container writes on the shared dev box). One divergence recorded:
  a permissions-read failure at the crons/secrets gate answers
  fail-closed 403 where TS would 500. The defs detail trio
  (`defs/$id`, `/edit`, `/versions`) stays TS for the next slice.

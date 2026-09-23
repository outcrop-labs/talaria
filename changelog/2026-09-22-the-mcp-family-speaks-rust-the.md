- **The mcp family speaks Rust — the org registry, the per-user connect
  plane, the OAuth pair, and the agent gateway that relays (or refuses)
  every tool call.** Eleven route files crossed: the per-agent roster
  (members get server names only), the registry pair with its
  create-and-sniff (a 401 challenge with resource metadata flips OAuth on),
  me/mcp connect/disconnect with credentials sealed at rest, the
  fleet-defs versioned MCP hook, the OAuth start/callback ladder, the test
  probe, the icon fallback, the library shelf, and `/api/mcp/gw/{server}`
  POST+GET — registered on the streaming router, because a legitimate
  tools/call holds up to 120s and the GET is a live SSE stream. The
  gateway keeps the family's security lines: identity resolved from the
  credential (never the claimed name), the caller's OAuth token spent only
  through the effective-server resolver, secret handles resolved on the way
  OUT and never on results, the per-assignment tools gate, in-process
  dispatch for the workbench, and SSE tools/list filtering that
  re-serializes only `data:` frames. App servers stay TS by the port's
  rule 10 — a STAY_TS hole in the coexistence proxy carves
  `/api/mcp/gw/app-*` back out of the family's prefix. Verified by a
  75-case byte-diff against TS with every network touch pinned to a
  deterministic answer (loopback refused, `.invalid` DNS, and the real
  loopback services: the builtin toolkit relay and the workbench's
  in-process dispatch). The diff caught five port bugs the unit tests
  could not: unaliased computed columns in the registry's row read (every
  full-row read 500'd — sqlx's derived FromRow matches by output column
  name), `created_by` bound with a uuid cast against a text column (every
  created agent version 500'd), zod's `Invalid URL` sentence casing, the
  icon route's bare 404 (no body, no content-type), and org discovery's
  transport errors, which must read undici's shapes (`fetch failed`) --
  reqwest's messages never reach a TS caller.

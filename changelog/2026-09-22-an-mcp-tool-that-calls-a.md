- **An MCP tool that calls a route the Rust router no longer serves fails
  `bun run check`.** The MCP server binds the api by hardcoded path strings
  (`api('GET', '/api/boards')` in mcp/src) — until now an api route rename
  became a runtime 404 inside a live agent session, discovered by whoever's
  agent went quiet. `scripts/check-mcp-routes.mjs` (the new fourth link in
  the `check` chain) reads the router table through the same parse gen-docs
  generates docs/api from — `scripts/route-table.mjs`, extracted verbatim
  from gen-docs with byte-identical output as the invariant of the move —
  and resolves every `api()` call site, the uploads raw-fetch, and the
  verify probe's default path against it, method included: 61 api() calls
  and the fetch all resolve against 251 routes today. The matcher is
  conservative (template holes match one path segment, never reaching across
  a `/`; nested template literals like `${q ? \`?q=${…}\` : ''}` parse
  brace-aware); what it cannot decide it fails, and exceptions argue in the
  ALLOWLIST, not by widening the pattern.
  Verified: gen-docs output byte-identical after the extraction (`git
  status docs/` empty after regen; `--check` green); the cross-check green
  on the tree and RED naming `mcp/src/index.ts:290` when one path was
  deliberately mutated, green again after restore; `bun run check` green
  with the new link wired in.

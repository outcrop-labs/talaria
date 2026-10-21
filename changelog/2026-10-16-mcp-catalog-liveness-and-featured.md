- **The MCP marketplace hides servers that no longer answer, and admins edit the featured
  shelf.** The catalog mirror carries every hosted entry the registry lists (~28k of ~42k), and
  a good share are gone — weekend demos, expired tunnels, hosts that 404 — so an install failed
  at discovery after the person had already chosen it. A new leased job,
  `mcp-library-liveness` (every 5 min, first run 3 min after boot), POSTs one JSON-RPC
  `initialize` to up to 300 hosted entries a run (never-checked first, operator stand-ins ahead
  of registry rows, then the oldest verdict; 8 in flight, 6 s each, at most 3 redirects, through
  the SSRF guard's resolver) and records `ok` (a JSON-RPC result or SSE data), `auth`
  (401/403/407 — alive, wants credentials), `dead` (404/405/410, 5xx, DNS/connect/TLS failure,
  timeout) or `unknown` on the row (`live_state`, `live_checked_at`, `live_detail`; appended
  migration). Search sorts dead rows last and leaves them out by default, answering
  `hiddenDead` — the picker shows "12 unreachable servers hidden · show" (`includeDead=1`), and a
  shown dead card says "Unreachable when last checked 3h ago" with the reason on hover and still
  installs. Every server carries `live: {state, checkedAt, detail}`; the featured shelf never
  shows a dead row; the status line adds "N checked · N unreachable · N to go"
  (`catalog.liveness`, partitioned on the verdict so unreachable never exceeds checked). Search
  runs the page and the hidden-dead count as two statements side by side (a partial index on
  the dead rows; appended migration), so the filter-only view costs what it did before. A row the sync rewrites with a changed entry is re-asked on the next run
  (and loses its verdict when its endpoint changed). Admins can run a batch now:
  `POST /api/mcp/library/liveness`. The featured shelf was a Rust constant; it is now the
  built-in list merged with the admin's edits (`app_settings.mcp_featured`: added domains,
  hidden built-ins, and stand-ins `{domain, title, url}` that win over the registry and the
  documented map for their publisher), edited from the marketplace's **Edit featured** link
  (`GET`/`PUT /api/mcp/library/featured`, admin only, bare hosts and https URLs validated —
  a stand-in URL with a user name, password, query string or fragment is refused, since every
  member is served it — audited as `mcp.featured`); a save shows on the shelf at once. `TALARIA_MCP_REGISTRY_URL`
  points the mirror at another registry list endpoint (an e2e stub). Verified: cargo tests for
  `classify_liveness` (every status class and failure kind), the merged featured list (add, hide
  a built-in, stand-in override, validation), the shelf picker and search ordering with dead
  rows, the status and hit wire shapes, the job timings and an unbound loopback port judged
  dead; the transport-error mapping and SSE framing under a non-SSE content type; a panicking
  batch not wedging the in-flight flag; `#[ignore]`d live-DB tests (`catalog_live`, run against
  the worktree database in a throwaway schema, and failing when the dead-last key is flipped,
  the dead filter dropped or `checked` counted by stamp) for search's dead-last order, the
  hidden-dead count under limit/kind/tier filters, the all-dead empty page, the shelf passing
  over dead publisher rows, and the status counts through a same-URL and a changed-URL rewrite;
  the new search statements timed against the worktree mirror with 1 in 5 hosted rows marked
  dead (empty query, kind=hosted: page 81–98 ms as before, dead count 3 ms; "git": page
  ~855 ms against 940 ms before, count 12 ms); vitest for the liveness sentences and the editor's chip logic; svelte-check; every new
  query run against a copy of the worktree mirror (187 hosted rows probed in 10 s: 80 ok, 64
  auth, 41 dead, 2 unknown; dead stand-in hidden and counted, live one shown, includeDead
  flagging it last, a changed URL clearing its verdict); and the marketplace and editor
  rendered in the worktree app.

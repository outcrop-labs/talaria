# MCP — org-wide tools for agents, governed

Talaria manages MCP (Model Context Protocol) servers **for the whole org**: register a server once,
decide which agents carry it and which tools each may use, gate which people may exercise it through
agents acting for them — and have all of it **enforced at a gateway**, not suggested by config.

## The registry (Manage → MCP)

Each server row holds identity (name, label, description), transport (url, headers, timeout), and
access rules. Four kinds of rows:

- **Registered servers** — anything you add by URL or from the marketplace.
- **The built-in Talaria toolkit** — Talaria's own tools (tickets, documents, knowledge, channels,
  research, media). Every agent carries it; you govern per-agent/per-person tool subsets, but its
  identity and lifecycle are the platform's.
- **App-published servers** (badged **app**) — a [Talaria app](./APPS.md)'s `mcp.ts` tools. Same
  governance; lifecycle follows the app; calls dispatch in-process. The catalog comes from the
  app's build: its load probe checks each tool and leaves out (and names, on the server's card)
  any that fails, so the row carries its tools the moment the app is enabled and **Discover
  tools** answers that catalog. An app that is turned off — by an admin, or by a build that broke
  — loses its row and the agents carrying it roll; the gateway refuses its tools meanwhile. App
  tools are never picked as an org supplier (web search and the like): the platform's own caller
  cannot reach them.
- **Per-user servers** — servers where each person connects their own account (below).

## Enforcement: the gateway

Agents never see an upstream URL or credential. Their rendered configs point at
`host.docker.internal:<app port>/api/mcp/gw/<server>` — the app's own port (5273 by
default), through the gateway in the app process, not the built-in toolkit's standalone
listener on 5280. The fleet key + agent name identify the caller, and the gateway:

- resolves **effective access** = the agent's assignment ∩ (for personal assistants) the owner's
  allowance;
- **filters `tools/list`** down to the allowed set and **rejects `tools/call`** outside it; an
  agent holding a tool subset is also refused `resources/*`, `prompts/*` and `completion/*`
  (JSON-RPC `-32601`, recorded as denied), while an agent with no subset relays them as before;
- injects the right credentials upstream (org headers, org OAuth token, or the acting user's
  connected account) — a hand-edited agent config can never exceed what the registry granted;
- **binds upstream sessions to the agent they were minted for**: the `mcp-session-id` an
  upstream's `initialize` answer carries is recorded against the calling agent (Redis,
  `talaria:mcp-session:v1:<server>:<sid>`, 24 h, refreshed on use), and a later request
  presenting a session recorded for another agent is refused with JSON-RPC `-32602` ("this
  session belongs to another agent") — two agents sharing an org credential look alike to the
  upstream, so the gateway is what keeps them out of each other's sessions; a session the
  gateway never saw minted passes through, since an upstream can mint one out of band;
- tells **"no access"** from **"not connected"**. A server the agent may not use answers 403
  `no access to this MCP server`. A server it *may* use but whose account nobody connected yet
  (a per-user server whose owner has not connected, an org OAuth server nobody connected, a
  per-user server granted to an org agent) is answered in-house: `initialize` succeeds (so the
  harness keeps the server), `tools/list` answers the cached catalog filtered to the agent's
  allowance (a `connection_required` stand-in when the catalog is empty), and every call answers
  JSON-RPC `-32001` with one sentence the agent can repeat — "Your Notion account isn't connected
  to Talaria. Connect it at `<instance>/settings/connections`, then try again.", "The workspace's
  Linear connection isn't set up yet. An admin connects it on the MCP page at
  `<instance>/mcp/external`.", or "Notion is a per-user server; only a personal assistant can use
  it." The link is the verified hosting domain when set, else `AUTH_PUBLIC_URL`, else the
  request origin as the person's browser opens it (`host.docker.internal`, the agent's alias for
  a host-run app, becomes `localhost`). The in-house answers keep JSON-RPC's rules: a message
  without an `id` is a notification (202, no body), a batch answers an array, and an unknown
  protocol revision is negotiated down rather than echoed. Such servers stay in the agent's
  rendered config (an org agent carries a per-user server only when granted to it by name), the
  agent's MCP tab shows exactly what that config lists (`withheld` names a grant the config
  leaves out), and connecting rolls every agent that carries the server — team grants included —
  onto the live upstream.

Registry changes that alter what a *running* agent carries trigger **blue/green rolls** (Hermes
wires MCP at process start): the replacement container comes up healthy before cutover, replies
drain, nobody's conversation dies. Tool-subset changes enforce per-call and need no roll.

## Access rules

Per server:

- **Agents** — all-agents, or an explicit assignment per agent; either way each row can narrow to a
  tool subset ("All tools" is the explicit default). An agent on a team also carries that team's
  MCP grants (union with its own assignment; a team grant counts as assignment on scoped servers).
- **People** — per-person allow/deny plus optional tool subsets. For agents acting on someone's
  behalf (personal assistants), the person's allowance intersects the agent's.
- **Teams** — the same people-plane rules as a person (`mcp_team_access`). A personal assistant
  intersects its owner's allowance after team grants union in; a person's own deny still wins.

## Auth

- **Org headers** — static credentials (API keys, auth headers), SecretBox-sealed at rest as one
  JSON object (`mcp_servers.headers_sealed`; the legacy plaintext `headers` column stays `{}`),
  opened only by `talaria-mcp`'s `org_headers` module — for the gateway's upstream call,
  discovery and the platform's own tool calls. Their values never leave the api: the registry
  answers `headerKeys` (names only), the audit log records names only, and the Credentials editor
  writes a merge. A seal the instance cannot open (a database restored onto other key material)
  lists no keys, and the next Credentials save replaces it rather than failing. Rows written
  before sealing are sealed once the instance has been up past the old slot's drain
  (`TALARIA_ROLL_DRAIN_SECONDS` + 180s, because a draining old binary reads only the plaintext
  column), then again on every registry read — idempotently and safely across instances; DEK
  rotation re-seals them with the rest.
- **OAuth 2.1** — discovered from the server's 401 challenge: protected-resource metadata →
  RFC 8414 authorization-server metadata (path-aware) → dynamic client registration → PKCE.
  Providers without DCR (e.g. GitHub), or whose DCR refuses hosted callbacks (Vercel), get a
  manual OAuth-app flow: the provider's developer portal and the steps that matter there
  (`talaria-mcp-oauth/src/guides.rs`, keyed by domain), the **one canonical callback URL** (the
  verified hosting domain when set, else the request origin — computed by the api and shown
  verbatim, never by the browser), client id/secret, and an optional explicit scope list.
  Every discovered endpoint (resource metadata, authorization server, registration, authorize,
  token) is pinned to the server's own registrable domain **and must be `https://`** — a
  plain-http endpoint on the right domain is refused with a sentence naming it, the one
  exception being a loopback host the operator admitted through `TALARIA_FETCH_ALLOW_HOSTS`,
  which is how a stub on the same machine registers.
  Discovery re-runs on demand (**Re-detect auth**) and automatically when a header server starts
  answering 401 with OAuth metadata. Tokens are sealed per subject (`org` or a user), refreshed
  silently, and a revocation forces a visible reconnect.
- **Per-user servers** — each person connects their own account under Settings → Connections
  (OAuth or credential form driven by the server's declared headers). An assistant carries the
  server either way; until its owner connects, every call answers with the sentence that says
  where to connect (see the gateway above).

The instance's verified **hosting domain** (Admin → Org) is the canonical base for OAuth callbacks —
see [ONBOARDING.md](./ONBOARDING.md).

## The marketplace

Manage → MCP's add flow (and an agent's MCP tab) browses a **local mirror of the official MCP
registry**. The scheduler job `mcp-library-refresh` walks the registry once
(`/v0.1/servers`, cursor-paged, latest versions only) into `mcp_library_entries`, then pulls
`updated_since` incrementals every 30 minutes; a sync can also be started from the picker
("Sync now") or `POST /api/mcp/library/sync`. Search is Postgres full text over title, publisher
domain, registry name and description, ranked brand match → title prefix → tier (official,
verified, community) → text rank, with hosted/package and tier filters. The featured shelf is the
same query over an editorial list of business publishers; publishers the registry lacks are
resolved once per day through `/.well-known/mcp.json` or a documented endpoint (GitHub) and stored
as `well-known` / `curated` rows. The live registry is consulted only while the first walk is
still running — it stalls for tens of seconds at a time, which is why the mirror exists.

**Liveness.** A good share of the registry's hosted entries no longer answer. The leased job
`mcp-library-liveness` (every 5 minutes, first run 3 minutes after boot) takes up to 300 hosted
rows — never-checked first, operator stand-ins ahead of registry rows, then the oldest verdict —
and POSTs one JSON-RPC `initialize` to each (6 s, `Accept: application/json, text/event-stream`,
at most 3 redirects, through the SSRF guard's resolver), recording `live_state` on the row:
`ok` (a 2xx JSON-RPC result or SSE data), `auth` (401/403/407 — alive, wants credentials),
`dead` (404/405/410, 5xx, DNS/connect/TLS failure, timeout, an address the guard refuses) or
`unknown` (anything else). Search sorts dead rows last and leaves them out by default, answering
`hiddenDead` so the picker can offer "N unreachable servers hidden · show" (`includeDead=1`);
every server carries `live: {state, checkedAt, detail}` (null = never checked); the featured
shelf never shows a dead row. A row the sync rewrites with a changed entry loses its check stamp
(and, if its endpoint changed, its verdict), so it is re-asked on the next run. The catalog
status carries `liveness: {checked, dead, unknown, pending}`, partitioned on the verdict:
`checked` is every row with one (a rewritten row keeps its same-URL verdict until the re-check),
`dead` and `unknown` are subsets of it, and `pending` is the hosted rows with none yet. Admins
can run a batch now with `POST /api/mcp/library/liveness`. The dead-last order, the hidden count
and the status counts are proven against a real database by the `#[ignore]`d
`talaria-mcp-library` test `catalog_live` (`DATABASE_URL=… cargo test -p talaria-mcp-library
--test catalog_live -- --ignored`).

**The operator's featured list.** The shelf is the built-in publisher list merged with the
admin's edits in `app_settings.mcp_featured` — `{domains, hidden, standIns: [{domain, title, url,
description?}]}` — edited from the marketplace's **Edit featured** link (`GET`/`PUT
/api/mcp/library/featured`, admin only, audited as `mcp.featured`). Added domains join the shelf
after the built-ins; `hidden` takes any of them (built-ins included) off it; a stand-in features
its domain by itself and wins over the registry's entry and the documented map for that
publisher (stored as a `source = 'operator'` row, `op:<domain>`). Domains must be bare hosts and
stand-in URLs https with no user name, password, query string or fragment — every member who
opens the marketplace is served the URL and the audit row keeps it, so a key belongs in the
installed server's Credentials form, not the URL. A save applies to the shelf at once: stand-in rows are written before the PUT
answers, the well-known/curated pass re-runs on the sync it triggers.

**Pointing the mirror elsewhere.** `TALARIA_MCP_REGISTRY_URL` (read once at boot) replaces the
official list endpoint — an end-to-end suite runs the mirror against a stub registry this way.
It goes through the same SSRF guard, so a loopback stub also needs `TALARIA_FETCH_ALLOW_HOSTS`.

Installing runs discovery in the same step. A server that answers 401 asks for its credentials
right there (the registry's header declarations drive the form when it has them; free header
rows otherwise), a server that speaks OAuth offers Connect, and one whose provider refuses to
self-register shows the OAuth-app setup. Every server card carries the last discovery verdict
(`probe`: ok, auth-required, unreachable, error, with the server's own words) and a
**Credentials** editor that merges keys (`mergeHeaders`: set/remove, never echo) and re-tests.

## Health checks

Discovery runs when a person asks, so a revoked key or a dead host used to keep reading
"ready". The `mcp-health` job (`talaria-mcp` `health.rs`; hourly, first run two minutes after
boot, leased) re-asks every enabled **external** server — builtin, app and Workbench rows are
this deployment's own; an oci-http package is asked at its container (resolved, and started if
it stopped, the way the gateway's first call would), a stdio package not at all — and
`POST /api/mcp/health` (admin) runs the same pass on demand and answers
`{checked, changed, down, recovered, notified, servers}`. Passes in one process run one at a
time; each server's conversation gets 45 seconds whatever its own timeout, and each verdict is
written as it arrives, so one hung server delays only its own line:

- **org + headers** — `initialize` + `tools/list` with the sealed org headers. A changed tool
  *name* set replaces the cached catalog (the probe's drift line names at most twenty tools a
  side and counts the rest); assignments whose tool subset names a tool no longer
  offered land in the probe as `staleAssignments: [{agent, missing}]`, and the agent's MCP tab
  answers `assignedToolsMissing` per server (computed live against the catalog) with a one-click
  **Drop missing tools**. The tab's copy of the probe leaves `staleAssignments` out: it names
  every agent on the server, and an agent's reader may not read the others.
- **org + OAuth** — the org grant is refreshed first; a grant that was connected and that the
  provider refused to refresh is the `reconnect` probe state, and the card offers **Reconnect**.
- **per-user** — an unauthenticated handshake proves the host answers (a 401 counts), then each
  person's stored OAuth grant is refreshed; a refused one is flagged in `mcp_oauth_reconnect` and
  Settings → Connections shows **Reconnect** (`needsReconnect` on `GET /api/me/mcp`).

A server the last pass saw `ok` that this pass sees unreachable, error, auth-required or
reconnect files one in-app notification per admin (`mcp_server_down`, the capability-gaps
class) — once per transition: the pass compares against `mcp_servers.health_state` (what the
last *pass* saw), so a person's Check now cannot swallow it, and the swap is one locked update.
Recovery writes the probe only.

**Refusals are recorded where they happen.** `oauth_token_for` drops a grant whose refresh the
provider answers 400/401 and, in the same breath, flags it in `mcp_oauth_reconnect` — so a grant
the *gateway* lost mid agent call reads `reconnect` / `needsReconnect` at the next pass, not as a
first-time Connect; a fresh sign-in clears the flag. The refresh is **single-flight per grant**:
it runs under the token row's lock and re-reads the row once it holds it, so the gateway, the
health pass and a second pass refreshing one grant at once make one refresh — a provider that
rotates refresh tokens (the MCP auth spec requires it for public clients) would otherwise answer
the loser's spent token 400 and the loser would delete the winner's fresh grant. Both are proven
against Postgres by `talaria-mcp-oauth`'s ignored `refresh_live` test. While any external server's last probe is not ok, admins get one
"N MCP servers need attention" warning in `/api/alerts`. Every card says when the server was
last checked, with **Check now** (the discovery `PUT {refreshTools: true}`).

## An agent's MCP tab

Agent settings → MCP is the same registry seen from one agent: every server it carries and why
(org-wide, assigned, via a team, the Developer switch), a tool subset per assignment, the
servers it could carry with one toggle, and the marketplace / custom form for whoever may
install (`agents.manage`) — installing and assigning in one step. Assigning what already exists
is the agent's managers' call (`GET`/`POST /api/fleet/defs/{id}/mcp`). Raw endpoints are never
attached to an agent any more: the old `add` is refused, and leftover raw entries from before
the gateway are listed for removal.

## Guardrails context

The agent-facing toolkit still enforces the protocol-layer rules ([PRODUCT.md](./PRODUCT.md)):
no assign tool, no complete tool — agents report up to quality review and a human closes. MCP
governance adds the *external-tool* dimension: which outside capabilities each agent and person
gets, with the same auditability as everything else (every registry mutation is audit-logged).


## The workbench surface

`workbench` is a Talaria-owned server in this same registry, in-process like app surfaces, **not** all-agents: access is each agent's **Developer Agent** switch, and nothing else (assignment and team rows are refused for it). Its tools are the governed execution lifecycle (`doctor`, `list_repos`, `start_job`, `job_status`, `merge_to_testing`, `finish_job`, `prepare_env`) (see [`WORKBENCH.md`](./WORKBENCH.md)). The coding harness (Oh My Pi) authenticates through Talaria's gateway; the agent's grants from THIS registry are rendered into its MCP config at render time (the pass-through), so a sandboxed harness sees exactly the same governed tool world the agent does, with zero in-sandbox reconnection.

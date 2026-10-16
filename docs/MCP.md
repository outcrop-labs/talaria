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
  governance; lifecycle follows the app; calls dispatch in-process.
- **Per-user servers** — servers where each person connects their own account (below).

## Enforcement: the gateway

Agents never see an upstream URL or credential. Their rendered configs point at
`host.docker.internal:<app port>/api/mcp/gw/<server>` — the app's own port (5273 by
default), through the gateway in the app process, not the built-in toolkit's standalone
listener on 5280. The fleet key + agent name identify the caller, and the gateway:

- resolves **effective access** = the agent's assignment ∩ (for personal assistants) the owner's
  allowance;
- **filters `tools/list`** down to the allowed set and **rejects `tools/call`** outside it;
- injects the right credentials upstream (org headers, org OAuth token, or the acting user's
  connected account) — a hand-edited agent config can never exceed what the registry granted.

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

- **Org headers** — static credentials, sealed (envelope-encrypted) at rest, injected by the
  gateway, never echoed to the UI.
- **OAuth 2.1** — discovered from the server's 401 challenge: protected-resource metadata →
  RFC 8414 authorization-server metadata (path-aware) → dynamic client registration → PKCE.
  Providers without DCR (e.g. GitHub), or whose DCR refuses hosted callbacks (Vercel), get a
  manual OAuth-app flow: the provider's developer portal and the steps that matter there
  (`talaria-mcp-oauth/src/guides.rs`, keyed by domain), the **one canonical callback URL** (the
  verified hosting domain when set, else the request origin — computed by the api and shown
  verbatim, never by the browser), client id/secret, and an optional explicit scope list.
  Discovery re-runs on demand (**Re-detect auth**) and automatically when a header server starts
  answering 401 with OAuth metadata. Tokens are sealed per subject (`org` or a user), refreshed
  silently, and a revocation forces a visible reconnect.
- **Per-user servers** — each person connects their own account under Settings → Connections
  (OAuth or credential form driven by the server's declared headers). An assistant only carries the
  server once its owner has connected.

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

Installing runs discovery in the same step. A server that answers 401 asks for its credentials
right there (the registry's header declarations drive the form when it has them; free header
rows otherwise), a server that speaks OAuth offers Connect, and one whose provider refuses to
self-register shows the OAuth-app setup. Every server card carries the last discovery verdict
(`probe`: ok, auth-required, unreachable, error, with the server's own words) and a
**Credentials** editor that merges keys (`mergeHeaders`: set/remove, never echo) and re-tests.

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

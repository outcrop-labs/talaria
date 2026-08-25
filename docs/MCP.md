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
  tool subset ("All tools" is the explicit default).
- **People** — per-person allow/deny plus optional tool subsets. For agents acting on someone's
  behalf (personal assistants), the person's allowance intersects the agent's.

## Auth

- **Org headers** — static credentials, sealed (envelope-encrypted) at rest, injected by the
  gateway, never echoed to the UI.
- **OAuth 2.1** — discovered from the server's 401 challenge: protected-resource metadata →
  RFC 8414 authorization-server metadata (path-aware) → dynamic client registration → PKCE.
  Providers without DCR (e.g. GitHub) get a manual OAuth-app flow with the callback URL to copy and
  a link to the provider's app portal. Tokens are sealed per subject (`org` or a user), refreshed
  silently, and a revocation forces a visible reconnect.
- **Per-user servers** — each person connects their own account under Settings → Connections
  (OAuth or credential form driven by the server's declared headers). An assistant only carries the
  server once its owner has connected.

The instance's verified **hosting domain** (Admin → Org) is the canonical base for OAuth callbacks —
see [ONBOARDING.md](./ONBOARDING.md).

## The marketplace

Manage → MCP's add flow searches the official MCP registry (latest schema), ranked so real,
remote-capable servers surface first, with brand icons, a featured shelf of business tools, and
publisher resolution (`/.well-known/mcp.json`, documented endpoints) for names the registry lacks.
Servers requiring credentials or OAuth get the setup treatment automatically from their metadata.

## Guardrails context

The agent-facing toolkit still enforces the protocol-layer rules ([PRODUCT.md](./PRODUCT.md)):
no assign tool, no complete tool — agents report up to quality review and a human closes. MCP
governance adds the *external-tool* dimension: which outside capabilities each agent and person
gets, with the same auditability as everything else (every registry mutation is audit-logged).


## The workbench surface

`workbench` is a Talaria-owned server in this same registry — in-process like app surfaces, **not** all-agents: access is an explicit per-agent grant. Its tools are the governed execution lifecycle (`doctor`, `list_repos`, `start_job`, `job_status`, `merge_to_testing`, `finish_job`) — see [`WORKBENCH.md`](./WORKBENCH.md). Chosen coding harnesses that can serve MCP (Claude Code, Codex) additionally register as stdio servers on the agent's own Hermes config, and the agent's grants from THIS registry are rendered into each harness's native MCP config at render time (the pass-through) — so a sandboxed harness sees exactly the same governed tool world the agent does, with zero in-sandbox reconnection.

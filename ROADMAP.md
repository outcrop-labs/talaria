# Talaria roadmap & status

Talaria is the nerve center for a lean, agent-powered business: one workspace where your people and your
AI agents share the same surfaces (boards, chats, plans, design, finance, code) and run the company
together in real time, with sensible human-in-the-loop guardrails. Talaria has its own UI (the app in
[`ui/`](./ui)), its own Postgres/Redis state, and every agent is a full
[Hermes](https://github.com/outsourc-e/hermes-workspace) agent.

> ⚠️ **Work in progress, not production ready.** Shipping today: the PM suite, chat + channels with plan
> chat, the fleet engine and full agent harness (versioned internals included), the token ledger with
> auto-priced costs, and the ops surfaces. Kick the tires, follow along, but don't bet your business on
> it yet.

**Status legend:** ✅ shipped · 🚧 in progress · 🔭 planned.

## Shipped today ✅

The project management suite, the fleet engine, and auth are live and running:

- **PM suite** ✅. Plane/Linear-grade boards (personal or team), a single Board settings modal, and a
  restrictive per-board agent policy (you opt in to which agents can touch a board). List and kanban
  views; drag-and-drop; list columns you can show/hide, reorder, and sort.
- **Rich tickets** ✅. WYSIWYG markdown description, comments (Ctrl+Enter to send), activity, watchers, a
  review gate, effort (XS to XL), multiple assignees, dependencies, a blocked lane, time that adds itself
  up, and links straight to any ticket. Lanes: Inbox, Assigned, In progress, Blocked, Quality review,
  Done.
- **Multiplayer** ✅. Every surface is live over Redis pub/sub and SSE. Teams and members baked in.
- **Fleet engine (gateway plane)** ✅. A gateway plane on `:8642` multiplexes your Hermes fleet so Talaria
  reaches every agent through one endpoint: `/v1/models` is the whole fleet, `/v1/chat/completions` routes
  by model to the right agent's real gateway (per-agent key, SSE streamed).
- **Redis-backed auth** ✅. A pluggable, env-gated provider registry (Google OAuth or username/password),
  stateless HMAC sessions, and a login screen that renders only what's enabled.
- **Guardrails** ✅. Agents can create and triage work, but they can't assign it to themselves or mark
  their own work done. Assigned and done stay a human's call; agents report up to `quality_review`.
- **Agent MCP (`talaria-mcp`)** ✅. An MCP server ([`mcp/`](./mcp)) exposing only the safe tools — list,
  read, create (into inbox), triage, comment, report outcome, log time, link dependencies. No assign tool,
  no complete tool: the guardrails hold at the protocol layer, and each agent only sees boards whose
  policy allows it.
- **Group chat** ✅. Slack-style channels where teammates and fleet agents are real members. @mention an
  agent (or `@agent:tier` to pick a model tier) and its reply streams into the channel for everyone, live
  over SSE. Composer autocompletes mentions; channel settings manage people and agents.
- **Plan chat** ✅. Hit **Plan** in a channel: an agent drafts tickets from the conversation, you review,
  edit, and create the keepers onto any board — into inbox, never assigned.
- **Agent harness** ✅. Talaria is the fleet's source of truth: import an existing Hermes stack, render and
  orchestrate every agent (`docker compose` project owned by Talaria), edit souls/models/tiers/fallbacks
  and MCP servers in-app as immutable, revertible config versions, create new agents from templates, and
  retire them — all from `/agents`, `/models`, and `/mcp`.
- **Versioned agent internals** ✅. Soul, model config, and MCP servers are version-controlled in-app
  (diffable history, reverts); skills and memory are managed live on the real mounts (`/skills`,
  `/memory`). Nothing shifts silently.
- **Token ledger + auto-priced costs** ✅. Every generation lands in the ledger, classified local vs cloud
  and attributed to the serving model/endpoint (tier-aware). Prices auto-fetch from a public catalog —
  zero config — with manual overrides; `/cost` shows spend, the local/cloud mixture, and per-agent $;
  agents report per-ticket token spend over MCP and tickets show tokens · $.
- **Ops surfaces** ✅. `/activity` (one merged workspace feed), `/alerts` (live-derived health: containers,
  gateway, ledger blind spots, stuck work), `/inference` (local backends probed live + local throughput).

## Next / planned

Roughly in the order we're chasing it. Full detail in
[`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md).

- **Design and creative** 🔭. Agents and humans making creative work side by side.
- **Finance** 🔭. Agent and human finance that plugs into the big accounting and HR platforms.
- **Agentic coding** 🔭. Agent-driven coding right in the app (probably
  [opencode](https://github.com/sst/opencode) wired into the UI).
- **Personal + base agents** 🔭. Role-ready base agents pre-configured for common business roles (support,
  sales, finance, dev, ops), plus per-person Docker-based Hermes personal assistants with their own
  skills, tools, and memories.
- **Marketplace** 🔭. A per-agent marketplace for MCP servers, skills, and tools.
- **Connectors** 🔭. Chat connectors (Slack, Mattermost, Matrix) for outbound notifications and autonomous
  outreach, an MCP connector to pull Talaria's agents into desktop/terminal apps, and accounting/HR/ops
  connectors.
- **Business multitenancy** 🔭. Spin Talaria up for several companies and swap between them in a click.
  Run them all on one server, or connect out to other hosted Talaria instances.
- **Artifacts linked to tickets** 🔭. Spin up artifacts on the fly and pin them to the tickets they belong
  to, right from chats, plans, and work sessions.
- **Run your own LLMs** 🚧. Local backends already register alongside hosted providers (auto-classed by
  URL), get probed live on `/inference`, and split the cost view. Next: managing the inference containers
  themselves (Ollama, vLLM, llama.cpp) from Talaria.
- **Container orchestration** 🔭. A [Dokploy](https://dokploy.com)-style backend for the containers Talaria
  spins up (personal agents, services), self-hosted.
- **Analytics / ROI** 🚧. The ledger, auto-priced costs, local/cloud mixture, and per-ticket spend ship
  today; full analytics across work, chats, and projects (board rollups, trends, ROI) is next.
- **Permissions** 🚧. Real, fine-grained access control for agents and people, growing from the guardrails
  we already ship.
- **Output guardrails** 🔭. Platform-level confabulation/safety checks on agent output (today the
  structural confab-guard lives as a per-agent Hermes plugin, annotate-only). Needs the agents to export
  tool-call traces to Talaria; then a server-side claims-vs-actions annotator on replies and ticket
  outcomes, with library-backed semantic rails (NeMo Guardrails / Guardrails AI class) as a later layer.
- **Open source, free forever + managed cloud** 🔭. The whole thing is OSS and self-hostable (MIT); a
  managed cloud for busier companies comes later.
- **Other agent runtimes** 🔭. Every agent is a full Hermes agent today; other runtimes are planned.

## Fleet engine (legacy Phase-1 origin)

Talaria started life as a bridge in front of external tools, with milestones M0-M5 (verified live against
`scripts/verify-stack.sh`). That scaffolding is where the fleet engine came from:

- **Gateway plane (fleet multiplexer)** ✅ (**lives on as the current fleet engine**). One endpoint
  multiplexes the fleet: each agent is an OpenAI model, `/v1/chat/completions` routes by model to that
  agent's real gateway (per-agent key, SSE streamed). This is how Talaria reaches the fleet today.
- **Dashboard plane / mission-control bridge / conductor mission routing / mission-control adapter**
  (M0-M5), **legacy Phase-1 scaffolding**. These fronted a running hermes-workspace + mission-control:
  conductor missions and a kanban view served from mission-control, the plugin registering/heartbeating/
  reporting into MC, and a `HermesAdapter` making Hermes a first-class MC framework. Talaria no longer
  runs hermes-workspace or proxies a live mission-control (we lifted those capabilities into our own
  Postgres/Redis), so this bridge is kept for reference but is not Talaria's architecture or identity.

The M0-M5 detail and the two-plane bridge design live on in [`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md)
and the surrounding [`docs/`](./docs). For where the product is headed, see [`README.md`](./README.md).

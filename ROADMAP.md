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
- **Fleet engine (Talaria gateway)** ✅. Every agent routes its LLM **and** its persona chat through
  Talaria's own gateway; Talaria renders the fleet and reaches each agent directly on its published port.
  One `talaria` network, providers registered in-app, provider secrets encrypted in the DB — no bridge,
  no Dockerfiles.
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
- **Agent harness** ✅. Talaria is the fleet's source of truth: design new agents from a plain-language
  description (Muse drafts identity, soul, and starter skills for review), federate outside Hermes agents
  in as natives, render and orchestrate everything from one Talaria-owned chassis (`fleet/chassis.yml`,
  `docker compose` project owned by Talaria), edit souls/models/tiers/fallbacks and MCP servers in-app as
  immutable, revertible config versions, manage per-agent encrypted secrets and native Hermes cron
  schedules, and retire agents — all from `/agents` and `/models`.
- **Versioned agent internals** ✅. Soul, personality, model config, skills, and memory are all
  version-controlled in-app with full diff-and-restore workspaces — and Muse (the drafting AI, on each
  user's preferred model) iteratively writes or refines any of them. Nothing shifts silently.
- **Token ledger + auto-priced costs** ✅. Every generation lands in the ledger, classified local vs cloud
  and attributed to the serving model/endpoint (tier-aware). Prices auto-fetch from a public catalog —
  zero config — with manual overrides; `/cost` shows spend, the local/cloud mixture, and per-agent $;
  agents report per-ticket token spend over MCP and tickets show tokens · $.
- **Ops surfaces** ✅. `/activity` (one merged workspace feed), `/alerts` (live-derived health: containers,
  gateway, ledger blind spots, stuck work), `/inference` (local backends probed live + local throughput).

## Next / planned

Roughly in the order we're chasing it. Full detail in
[`docs/TODO.md`](./docs/TODO.md).

- **Design and creative** 🔭. Agents and humans making creative work side by side.
- **Finance** 🔭. Agent and human finance that plugs into the big accounting and HR platforms.
- **Agentic coding** 🔭. Agent-driven coding right in the app (probably
  [opencode](https://github.com/sst/opencode) wired into the UI).
- **Personal assistants** ✅ / **base agents** 🔭. Per-person Hermes assistants shipped: a guided
  onboarding wizard, owner-scoped controls (handle, model tier, personality, skills, memory, schedules),
  private knowledge, own container + key. Role-ready base agents for common business roles remain planned
  (Muse-designed agents cover much of the gap today).
- **Marketplace** 🔭. A per-agent marketplace for MCP servers, skills, and tools.
- **Connectors** 🔭. Chat connectors (Slack, Mattermost, Matrix) for outbound notifications and autonomous
  outreach, an MCP connector to pull Talaria's agents into desktop/terminal apps, and accounting/HR/ops
  connectors.
- **Business multitenancy** 🔭. Spin Talaria up for several companies and swap between them in a click.
  Run them all on one server, or connect out to other hosted Talaria instances.
- **Artifacts linked to tickets** ✅. Artifacts attach to tickets, chats, and channels; agents create and
  update them through the toolkit, with Google Drive export.
- **Run your own LLMs** 🚧. Local backends already register alongside hosted providers (auto-classed by
  URL), get probed live on `/inference`, and split the cost view. Next: managing the inference containers
  themselves (Ollama, vLLM, llama.cpp) from Talaria.
- **Container orchestration** 🔭. A [Dokploy](https://dokploy.com)-style backend for the containers Talaria
  spins up (personal agents, services), self-hosted.
- **Analytics / ROI** 🚧. The ledger, auto-priced costs, local/cloud mixture, and per-ticket spend ship
  today; full analytics across work, chats, and projects (board rollups, trends, ROI) is next.
- **Permissions** 🚧. Real, fine-grained access control for agents and people, growing from the guardrails
  we already ship.
- **Output guardrails** ✅ (first layer). The confab guard is Talaria-native at the LLM gateway: a
  pluggable rule registry (zero-tool claims, ungrounded refs, fabricated outages, secret leaks) with
  confidence scoring and off/observe/annotate/strict modes, deriving the turn's tool record from the
  request itself — no trace export needed. A QA judge (advisory or enforcing per board) reviews ticket
  outcomes with a bounded revision loop. Later: guard coverage on the direct chat path, and
  library-backed semantic rails (NeMo Guardrails / Guardrails AI class).
- **Open source, free forever + managed cloud** 🔭. The whole thing is OSS and self-hostable (MIT); a
  managed cloud for busier companies comes later.
- **Other agent runtimes** 🔭. Every agent is a full Hermes agent today; other runtimes are planned.

## Fleet engine (legacy Phase-1 origin)

Talaria started life as a bridge in front of external tools, with milestones M0-M5 (verified live against
`scripts/verify-stack.sh`). That scaffolding is where the fleet engine came from:

- **Gateway plane (fleet multiplexer)** ✅ → **retired**. The original bridge multiplexed the fleet on a
  single `:8642` endpoint. Once every agent routed its LLM and chat through Talaria's own gateway, the
  bridge was removed — Talaria now reaches each agent directly on its published port.
- **Dashboard plane / mission-control bridge / conductor mission routing / mission-control adapter**
  (M0-M5), **legacy Phase-1 scaffolding**. These fronted a running hermes-workspace + mission-control:
  conductor missions and a kanban view served from mission-control, the plugin registering/heartbeating/
  reporting into MC, and a `HermesAdapter` making Hermes a first-class MC framework. Talaria no longer
  runs hermes-workspace or proxies a live mission-control (we lifted those capabilities into our own
  Postgres/Redis), so this bridge is kept for reference but is not Talaria's architecture or identity.

The M0-M5 detail and the two-plane bridge design live on in [`docs/m0-contract.md`](./docs/m0-contract.md)
and the surrounding [`docs/`](./docs). For where the product is headed, see [`README.md`](./README.md).

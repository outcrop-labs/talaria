# Talaria roadmap & status

Talaria is the nerve center for a lean, agent-powered business: one workspace where your people and your
AI agents share the same surfaces (boards, chats, plans, design, finance, code) and run the company
together in real time, with sensible human-in-the-loop guardrails. Talaria has its own UI (the app in
[`ui/`](./ui)), its own Postgres/Redis state, and every agent is a full
[Hermes](https://github.com/outsourc-e/hermes-workspace) agent.

> ⚠️ **Work in progress, not production ready.** Only the PM suite, the fleet engine (gateway plane), and
> auth are shipped today. Almost everything else on this page is still on the way. Kick the tires, follow
> along, but don't bet your business on it yet.

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

## Next / planned

Roughly in the order we're chasing it. Full detail in
[`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md).

- **Agent MCP (`talaria-mcp`)** 🚧. Expose only the safe create/triage tools to agents over MCP, so the
  guardrails hold at the protocol layer. This is the immediate next thing.
- **Group chat** 🔭. Multi-agent and human channels, Slack-style, except your agents are real members.
- **Plan chat** 🔭. Humans and agents plan together, then turn the conversation into realistic tickets
  (missions) and drop them onto any connected kanban.
- **Design and creative** 🔭. Agents and humans making creative work side by side.
- **Finance** 🔭. Agent and human finance that plugs into the big accounting and HR platforms.
- **Agentic coding** 🔭. Agent-driven coding right in the app (probably
  [opencode](https://github.com/sst/opencode) wired into the UI).
- **Personal + base agents** 🔭. Role-ready base agents pre-configured for common business roles (support,
  sales, finance, dev, ops), plus per-person Docker-based Hermes personal assistants with their own
  skills, tools, and memories.
- **Versioned agent internals** 🔭. Each agent's soul, memory, skills, tools, and MCP servers are managed
  in-app and version controlled. Every change is tracked, diffable, and revertible, so nothing shifts
  under you silently. (We do manage your agents, we just do it in the open and always undoably.)
- **Marketplace** 🔭. A per-agent marketplace for MCP servers, skills, and tools.
- **Connectors** 🔭. Chat connectors (Slack, Mattermost, Matrix) for outbound notifications and autonomous
  outreach, an MCP connector to pull Talaria's agents into desktop/terminal apps, and accounting/HR/ops
  connectors.
- **Business multitenancy** 🔭. Spin Talaria up for several companies and swap between them in a click.
  Run them all on one server, or connect out to other hosted Talaria instances.
- **Artifacts linked to tickets** 🔭. Spin up artifacts on the fly and pin them to the tickets they belong
  to, right from chats, plans, and work sessions.
- **Run your own LLMs** 🔭. Manage local inference (Ollama, vLLM, llama.cpp, and friends) right alongside
  hosted models.
- **Container orchestration** 🔭. A [Dokploy](https://dokploy.com)-style backend for the containers Talaria
  spins up (personal agents, services), self-hosted.
- **Analytics / ROI** 🔭. Full agent analytics plus analytics across work, chats, and projects, with token
  cost and ROI running through all of it.
- **Permissions** 🚧. Real, fine-grained access control for agents and people, growing from the guardrails
  we already ship.
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

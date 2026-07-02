# Talaria 🪽👟

> *Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

**Talaria is the nerve center for a lean, agent-powered business.** It's a collaborative workspace where
your people and your AI agents share the same surfaces — boards, chats, plans, design, finance, code — and
run the day-to-day of a company together, in real time, with **sensible guardrails** that keep humans in
the loop on the decisions that matter. Every agent is a full [**Hermes**](https://github.com/outsourc-e/hermes-workspace)
agent (memory, skills, run loop intact), and Talaria is the one beautiful place your whole org — human and
agent — actually works.

You can keep every other tool you already use. Talaria isn't a walled garden — it connects out to the
ecosystems you rely on. But for the daily work of running the business, this is meant to be the single pane
of glass: **manage almost everything, in one place, with your agents beside you.**

> **Status legend:** ✅ shipped · 🚧 in progress · 🔭 planned. Talaria is early — the PM suite and the fleet
> engine are live today; the rest of this document is the deliberate destination. Milestones live in
> [`ROADMAP.md`](./ROADMAP.md) and [`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md); newcomers start with
> [`HANDOFF.md`](./HANDOFF.md).

## What makes it different

- **Agent + human, not agent *or* human.** Every surface is multiplayer and mixed: people and agents in the
  same board, the same chat, the same plan. Collaboration is the default, not a bolt-on.
- **Hermes-first.** ✅ Every agent is a real Hermes agent behind one gateway — no weaker "worker" tier.
  Alternative agent runtimes are 🔭 planned, but Hermes is the first-class citizen.
- **Sensible guardrails.** ✅ Agents create and triage work but can't self-assign or sign their own work
  off — assignment and approval stay human. True fine-grained permissions for **both agents and users** are
  🚧 expanding from there.
- **Open source, free forever.** 🔭 The full platform is OSS and self-hostable; a managed cloud service for
  mature businesses is coming for teams who'd rather not run it themselves.
- **Privacy-forward.** 🔭 Personal assistant agents track with content **stripped** (metadata only); public
  business agents keep full logs. You choose what's shared.
- **Prove the ROI.** 🔭 See in real time how your agentic stack contributes to business efficiency, with
  token-cost metrics tied to the work it produced.

## The workspace — where the work happens

- **Project management** ✅ — Plane/Linear-grade boards, rich tickets (effort, multiple assignees,
  dependencies, blocked state, auto-accumulated time), teams, watchers, a quality-review gate, list + kanban
  views, directly-linkable tickets, all live/multiplayer.
- **Group chat** 🔭 — multi-agent + human channels (think Slack, but your agents are first-class members).
- **Plan chat** 🔭 — a multi-agent + human planning surface that turns a conversation into **realistic
  tickets (missions)**, triaged across any connected kanban.
- **Design & creative suite** 🔭 — agent + human collaboration on creative work.
- **Finance suite** 🔭 — agent + human finance, connectable to major accounting and HR platforms.
- **Agentic coding** 🔭 — in-app coding driven by agents (likely [opencode](https://github.com/sst/opencode)
  surfaced through the UI).

## The agents — first-class citizens

- **Personal assistant agents** 🔭 — a Docker-spawned Hermes agent per user, connected to pre-configured
  company model profiles, that persists its own skills, tools, and memories.
- **Versioned agent identity** 🔭 — soul, memory, skills, and tools are version-controlled per agent and
  synced elegantly across the fleet.
- **Self-improving, lean** 🔭 — agents create their own skills/tools automatically, and unused ones are
  auto-archived to keep context lean.
- **Marketplace** 🔭 — a per-agent marketplace for MCP servers, skills, and tools.
- **Permissions** 🚧 — true, fine-grained access control for agents *and* humans.

## Connect everything

- **Chat connectors** 🔭 — configurable outbound connectors for notifications and **autonomous agent
  outreach**: Slack, Mattermost, Matrix, and more.
- **MCP connector** 🔭 — bring Talaria's agents into your desktop/terminal apps over MCP.
- **Accounting / HR / ecosystem integrations** 🔭 — plug the finance and ops suites into the platforms you
  already run.

## Under the hood — the fleet engine

Beneath the product sits Talaria's fleet runtime: **two planes** in front of your agents. You declare a fleet
once and Talaria routes to it; every node stays a full Hermes agent. Talaria owns its **own** Postgres/Redis
as the system of record — mission-control's capabilities were ripped into our stack, **not** proxied.

```
                         Talaria UI  +  hermes-workspace
              gateway plane │            │ dashboard plane
                            ▼            ▼
        ┌──────────────────────────┐  ┌──────────────────────────────┐
        │ GATEWAY PLANE  :8642      │  │ DASHBOARD PLANE  :9119        │
        │ fleet multiplexer         │  │ management bridge             │
        │ • /v1/models = whole fleet│  │ • serves conductor + kanban   │
        │ • /v1/chat routed by model│  │ • proxies everything else     │
        │   → the right agent, per- │  │   through untouched           │
        │   agent key, SSE streamed │  │                               │
        └──────────────┬───────────┘  └───────────────┬──────────────┘
          per-agent     │                 register /   │
          routing       ▼                 heartbeat /  ▼
   ┌────────┬────────┬────────┐           report    Talaria's own Postgres/Redis
   ▼        ▼        ▼        ▼                       (boards, tickets, teams, cost,
 agent-1  agent-2  …      agent-N                     activity — owned, not proxied)
 gateway  gateway         gateway
   (each a full Hermes agent)
```

- **Full local LLM stack control** 🔭 — run and manage your own inference (Ollama, vLLM, llama.cpp, …)
  alongside hosted models.
- **Container orchestration** 🔭 — a [Dokploy](https://dokploy.com)-style backend for the containers Talaria
  spawns (personal agents, services), self-hosted.
- **Analytics everywhere** 🔭 — full agent analytics plus analytics across work, chats, and projects, with
  token-cost/ROI woven through.

### The pieces (today)

| Path | Piece | What it does |
|---|---|---|
| [`ui/`](./ui) | **Talaria app** (Vite + TanStack Start) ✅ | The product: boards, tickets, teams, multiplayer, auth. Owns state in Postgres/Redis. |
| [`bridge/`](./bridge) | **talaria-bridge** (Node/TS) ✅ | The fleet engine. Gateway plane multiplexes the fleet; dashboard plane bridges conductor + kanban, proxies the rest. |
| [`stack/fleet.json`](./stack/fleet.example.json) | **fleet manifest** | Declares your agents (model → gateway url + key). Gitignored (holds keys). |
| [`plugin/talaria/`](./plugin/talaria) | **Talaria Hermes plugin** ✅ | Rides on each agent: registers, heartbeats for work, reports toward `quality_review` (never `done`). |
| [`adapter/`](./adapter) | **mission-control adapter** ✅ | Makes Hermes a first-class framework inside mission-control (lift source). |
| [`stack/`](./stack) | **docker stack** ✅ | Compose wiring the fleet engine + network together. |

## What's live today

The **project-management suite + the fleet engine** are shipped and running:

- Shareable kanban boards (personal/team), consolidated Board settings, restrictive board-scoped agent policy.
- Rich tickets: WYSIWYG markdown description, comments (Ctrl+Enter), activity, watchers, quality-review gate,
  effort (XS–XL), multiple assignees, dependencies, auto-accumulated time-spent, directly-linkable routes.
- Inbox → Assigned → In progress → Blocked → Quality review → Done; drag-and-drop board + a configurable,
  reorderable, sortable list view.
- Multiplayer over Redis pub/sub → SSE. Teams + members. Redis-backed auth (Google OAuth + password).
- Agent guardrails: agents triage but can't self-assign or self-complete.

**Next up:** the **agent MCP** (`talaria-mcp`) exposing only the safe create/triage tools, then group chat
and the plan surface.

## Run it

**The app:**

```bash
cd ui
cp .env.example .env       # set AUTH_SECRET, enable a provider, point at Postgres/Redis
npm install
npm run dev                # http://localhost:5273
```

Dev state runs as containers (`talaria-postgres-dev` :5544, `talaria-redis-dev` :6399). Default self-host
admin: **`jon@packledger.co` / `talaria-dev`**. Details + gotchas in [`ui/README.md`](./ui/README.md) and
[`HANDOFF.md`](./HANDOFF.md).

**The fleet engine:**

```bash
cp stack/.env.example stack/.env
cp stack/fleet.example.json stack/fleet.json   # declare your agents (model → gateway url + key)
docker compose -f stack/docker-compose.yml up -d --build
./scripts/verify-stack.sh                       # should print ALL PASS
```

## The "don't break anything" promise

Talaria routes; it never rewrites your agents.

- **Every node stays a full Hermes agent** — chat/streaming forwarded to each agent's real gateway with its
  own key; memory, skills, run loop intact.
- **Nothing is written to your agents** — the fleet manifest (with keys) is gitignored + mounted read-only;
  the plugin no-ops until configured.
- **Human sign-off is never bypassed** — agents report to `quality_review`; the final `done` is a human call.
- **Allowlist, not denylist** — anything the dashboard plane doesn't recognize passes straight through.

## License & vision

Open source, **free forever** (MIT — see [`LICENSE`](./LICENSE)); a managed cloud service for mature
businesses is planned. Private for now, going public when it's ready. Missing a corner you'd need to run your
business here? That's the whole point — [open an issue](https://github.com/outcrop-labs/talaria/issues) and
help shape the nerve center.

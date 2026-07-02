# Talaria 🪽👟

> *Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

Talaria is the nerve center for a lean, agent-powered business. It's one workspace where your people and
your AI agents share the same stuff (boards, chats, plans, design, finance, code) and actually run the
company together, in real time, with sensible guardrails so a human stays in the loop on the calls that
matter. Every agent is a full [Hermes](https://github.com/outsourc-e/hermes-workspace) agent (memory,
skills, run loop, all intact), and Talaria is the one nice place your whole team, human and agent alike,
gets work done.

Keep every other tool you already love. Talaria isn't a walled garden, it talks to the ecosystems you
rely on. But for the day-to-day of running the business, the goal is simple: one place to manage almost
everything, with your agents right there beside you.

> **Status legend:** ✅ shipped, 🚧 in progress, 🔭 planned. Heads up: Talaria is early. The PM suite and
> the fleet engine work today; the rest of this is where we're headed. Milestones live in
> [`ROADMAP.md`](./ROADMAP.md) and [`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md). New here? Start
> with [`HANDOFF.md`](./HANDOFF.md).

## Why it's different

- **Agent and human, not one or the other.** Every surface is multiplayer and mixed. People and agents
  in the same board, the same chat, the same plan. Working together is the default, not a bolt-on.
- **Hermes first.** ✅ Every agent is a real Hermes agent behind one gateway. No weak "worker" tier.
  We'll add other agent runtimes later (🔭), but Hermes is the star.
- **Guardrails that make sense.** ✅ Agents can create and triage work, but they can't assign it to
  themselves or mark their own work done. That stays with a human. Fine-grained permissions for both
  agents and people are 🚧 growing from there.
- **Open source, free forever.** 🔭 The whole thing is OSS and self-hostable. A managed cloud for busier
  companies is coming for folks who'd rather not run it themselves.
- **Privacy first.** 🔭 Personal assistant agents track with the content stripped out (metadata only).
  Public business agents keep full logs. You decide what gets shared.
- **Show the ROI.** 🔭 Watch, in real time, how your agent stack moves the business, with token costs tied
  to the work it actually shipped.

## The workspace, where the work happens

- **Project management** ✅. Plane/Linear-grade boards, rich tickets (effort, multiple assignees,
  dependencies, a blocked lane, time that adds itself up), teams, watchers, a review gate, list and
  kanban views, tickets you can link straight to. All live and multiplayer.
- **Group chat** 🔭. Multi-agent and human channels. Think Slack, except your agents are real members.
- **Plan chat** 🔭. A place where humans and agents plan together, then turn the conversation into
  realistic tickets (missions) and drop them onto any connected kanban.
- **Design and creative** 🔭. Agents and humans making creative work side by side.
- **Finance** 🔭. Agent and human finance that plugs into the big accounting and HR platforms.
- **Coding** 🔭. Agent-driven coding right in the app (probably [opencode](https://github.com/sst/opencode)
  wired into the UI).

## The agents, treated like first-class team members

- **Personal assistants** 🔭. Spin up a Docker-based Hermes agent for each person, hook it to your
  company's model profiles, and let it keep its own skills, tools, and memories.
- **Versioned identity** 🔭. Each agent's soul, memory, skills, and tools are version controlled and
  synced across the fleet without the mess.
- **Self-improving and lean** 🔭. Agents build their own skills and tools, and the ones they stop using
  get auto-archived so context doesn't bloat.
- **Marketplace** 🔭. A per-agent marketplace for MCP servers, skills, and tools.
- **Permissions** 🚧. Real, fine-grained access control for agents and people.

## Connect it to everything

- **Chat connectors** 🔭. Wire up outbound notifications and autonomous agent outreach through Slack,
  Mattermost, Matrix, whatever you use.
- **MCP connector** 🔭. Pull Talaria's agents into your desktop and terminal apps over MCP.
- **Accounting, HR, and the rest** 🔭. Connect the finance and ops surfaces to the platforms you already
  run.

## Under the hood, the fleet engine

Underneath the app is Talaria's fleet runtime: two planes sitting in front of your agents. You declare a
fleet once and Talaria routes to it, and every node stays a full Hermes agent. Talaria keeps its own
Postgres and Redis as the source of truth. We ripped mission-control's capabilities into our own stack
instead of proxying a running copy.

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
 agent-1  agent-2  …      agent-N                     activity, owned not proxied)
 gateway  gateway         gateway
   (each a full Hermes agent)
```

- **Run your own LLMs** 🔭. Manage your own inference (Ollama, vLLM, llama.cpp, and friends) right
  alongside hosted models.
- **Container orchestration** 🔭. A [Dokploy](https://dokploy.com)-style backend for the containers
  Talaria spins up (personal agents, services), self-hosted.
- **Analytics everywhere** 🔭. Full agent analytics plus analytics across work, chats, and projects, with
  token cost and ROI running through all of it.

### The pieces (today)

| Path | Piece | What it does |
|---|---|---|
| [`ui/`](./ui) | **Talaria app** (Vite + TanStack Start) ✅ | The product: boards, tickets, teams, multiplayer, auth. Keeps state in Postgres/Redis. |
| [`bridge/`](./bridge) | **talaria-bridge** (Node/TS) ✅ | The fleet engine. Gateway plane multiplexes the fleet, dashboard plane bridges conductor and kanban and proxies the rest. |
| [`stack/fleet.json`](./stack/fleet.example.json) | **fleet manifest** | Lists your agents (model → gateway url + key). Gitignored, it holds keys. |
| [`plugin/talaria/`](./plugin/talaria) | **Talaria Hermes plugin** ✅ | Rides on each agent: registers, heartbeats for work, reports up to `quality_review` (never `done`). |
| [`adapter/`](./adapter) | **mission-control adapter** ✅ | Makes Hermes a first-class framework inside mission-control (lift source). |
| [`stack/`](./stack) | **docker stack** ✅ | Compose that wires the fleet engine and network together. |

## What works today

The project management suite and the fleet engine are shipped and running:

- Shareable kanban boards (personal or team), a single Board settings modal, and a restrictive per-board
  agent policy (you opt in to which agents can touch a board).
- Rich tickets: WYSIWYG markdown description, comments (Ctrl+Enter to send), activity, watchers, a
  review gate, effort (XS to XL), multiple assignees, dependencies, time that adds itself up, and links
  straight to any ticket.
- Inbox, Assigned, In progress, Blocked, Quality review, Done. Drag-and-drop board plus a list view with
  columns you can show/hide, reorder, and sort.
- Multiplayer over Redis pub/sub and SSE. Teams and members. Redis-backed auth (Google OAuth or
  password).
- Guardrails: agents triage but can't assign or complete their own work.

Next up: the agent MCP (`talaria-mcp`) that exposes only the safe create/triage tools, then group chat
and the plan surface.

## Run it

The app:

```bash
cd ui
cp .env.example .env       # set AUTH_SECRET, turn on a provider, point at Postgres/Redis
npm install
npm run dev                # http://localhost:5273
```

Dev state runs in containers (`talaria-postgres-dev` on :5544, `talaria-redis-dev` on :6399). The default
self-host admin is **`jon@packledger.co` / `talaria-dev`**. More detail and a couple of gotchas in
[`ui/README.md`](./ui/README.md) and [`HANDOFF.md`](./HANDOFF.md).

The fleet engine:

```bash
cp stack/.env.example stack/.env
cp stack/fleet.example.json stack/fleet.json   # list your agents (model → gateway url + key)
docker compose -f stack/docker-compose.yml up -d --build
./scripts/verify-stack.sh                       # should print ALL PASS
```

## The "don't break anything" promise

Talaria routes traffic, it never rewrites your agents.

- **Every node stays a full Hermes agent.** Chat and streaming go to each agent's real gateway with its
  own key. Memory, skills, run loop, all intact.
- **Nothing gets written to your agents.** The fleet manifest (with keys) is gitignored and mounted
  read-only, and the plugin does nothing until you configure it.
- **Human sign-off is never skipped.** Agents report up to `quality_review`. The final `done` is a
  human's call.
- **Allowlist, not denylist.** Anything the dashboard plane doesn't recognize passes straight through.

## License and vision

Open source, free forever (MIT, see [`LICENSE`](./LICENSE)). A managed cloud for busier companies is on
the way. Private for now, going public when it's ready. Missing something you'd need to actually run your
business here? That's the whole idea, so
[open an issue](https://github.com/outcrop-labs/talaria/issues) and help shape it.

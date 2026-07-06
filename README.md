# Talaria 🪽👟

> *Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

> ⚠️ **Work in progress, not production ready.** Talaria is under heavy active development. A lot of what's
> described below is still on the way, and things will change and break along the way. Kick the tires and
> follow along, but please don't bet your business on it yet.

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
- **Run many businesses from one place.** 🔭 Business multitenancy is coming soon, so you can spin Talaria
  up for several companies and swap between them in a click. Run them all on one server if you want, or
  connect out to other hosted Talaria instances.

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
- **Artifacts that stick around** 🔭. Spin up artifacts on the fly and pin them to the tickets they
  belong to, right from chats, plans, and work sessions. No more good work getting lost in a chat scroll.

## The agents, treated like first-class team members

- **Role-ready base agents** 🔭. Spin up pre-built agents made for common business roles (support, sales,
  finance, dev, ops, and so on) right from Talaria. They come already configured and ready to get to
  work, so you can hire a whole starter team in a few clicks and tweak from there.
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

Talaria has its own UI. We're building it by ripping the good parts out of hermes-workspace (mostly the
chat and agent UX) and wiring them into our own app. We do not run hermes-workspace, it's just a parts
bin we pull from. Same story with mission-control: we lifted its capabilities (task queue, cost,
activity) into our own Postgres and Redis instead of proxying a running copy. Talaria is the whole
surface now, and it owns its own state.

Underneath the app is the fleet runtime: a gateway plane that multiplexes your Hermes agents so Talaria
can talk to the whole fleet through one endpoint. You declare a fleet once, and every node stays a full
Hermes agent.

```
                     Talaria UI  (our own app)
                           │
                           ▼
         ┌──────────────────────────────┐
         │ GATEWAY PLANE  :8642          │
         │ fleet multiplexer             │
         │ • /v1/models = whole fleet    │
         │ • /v1/chat routed by model    │
         │   to the right agent, per-    │
         │   agent key, SSE streamed     │
         └───────────────┬──────────────┘
           per-agent      │                     Talaria's own Postgres/Redis
           routing        ▼                     (boards, tickets, teams, cost,
   ┌────────┬────────┬────────┐                 activity, owned not proxied)
   ▼        ▼        ▼        ▼                          ▲
 agent-1  agent-2  …      agent-N ──────────────────────┘
 gateway  gateway         gateway     register / heartbeat / report
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
| [`bridge/`](./bridge) | **talaria-bridge** (Node/TS) ✅ | The gateway plane: multiplexes the fleet (`/v1/models`, model-routed chat) so Talaria can reach every agent. (An old Phase 1 mission-control bridge lives here too, now legacy.) |
| [`stack/fleet.json`](./stack/fleet.example.json) | **fleet manifest** | Lists your agents (model → gateway url + key). Gitignored, it holds keys. |
| [`plugin/talaria/`](./plugin/talaria) | **Talaria Hermes plugin** ✅ | Rides on each agent: registers, heartbeats for work, reports up to `quality_review` (never `done`). |
| [`adapter/`](./adapter) | **mission-control adapter** ✅ | Makes Hermes a first-class framework inside mission-control (lift source). |
| [`stack/`](./stack) | **docker stack** ✅ | Compose that wires the fleet engine and network together. |

## What works today

The workspace, the fleet harness, and the money view are shipped and running:

- Shareable kanban boards (personal or team), a single Board settings modal, and a restrictive per-board
  agent policy (you opt in to which agents can touch a board).
- Rich tickets: WYSIWYG markdown description, comments, activity, watchers, a review gate, effort,
  multiple assignees, dependencies, time that adds itself up, per-ticket token spend, and links straight
  to any ticket. Drag-and-drop board plus a configurable list view; multiplayer over Redis pub/sub + SSE.
- Chat (1:1, any model tier) and Slack-style channels where agents are real members: @mention an agent —
  or `@agent:tier` — and the reply streams in live. **Plan** turns a conversation into reviewed tickets.
- The agent harness: import a Hermes stack, render and run every agent under Talaria's own compose
  project, edit souls/models/tiers/MCP servers as immutable revertible versions, manage skills and memory
  live, create agents from templates, retire them — all in-app.
- Knowledge & retrieval: an Outline-style markdown drive (`/knowledge`) — a searchable, nestable tree
  of docs with drag-to-reparent, emoji icons, breadcrumbs, an auto table-of-contents, and backlinks.
  Full-text search across the base; every doc is version-controlled (view + restore any revision);
  org/private/public visibility with public share links. The shared editor does task lists, tables, and
  images, round-tripping to markdown so agents read/write it cleanly. Marking a doc **official**
  indexes it into the **organization brain** agents ground on. A separate **workspace-activity brain**
  ambiently indexes chat/channels (and, over time, plans/research/tickets) as a `search_knowledge` tool
  agents call on demand — index-don't-copy, permission-scoped. Admins spin up custom **departmental
  collections** and bind each to specific users or agents (Admin → Retrieval). Qdrant + a self-hosted
  TEI embedder under the hood.
- The ledger: every generation classified local vs cloud, priced automatically from a public catalog
  (override anytime), per-agent and per-ticket spend, plus `/activity`, `/alerts`, and `/inference` for
  ops. The agent MCP (`talaria-mcp`) exposes only the safe board tools.
- Guardrails: agents triage but can't assign or complete their own work.

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

## Safe by design

Talaria does manage your agents, that's the point, but it does it carefully, in the open, and in a way
you can always undo.

- **Every agent is a real Hermes agent** ✅. Chat and streaming go to each agent's real gateway with its
  own key. No weak "worker" tier.
- **Version-controlled internals** 🔭. An agent's soul, memory, skills, tools, and MCP servers are
  managed right in the app and version controlled. Every change is tracked, diffable, and easy to roll
  back, so nothing shifts under you silently.
- **Human sign-off is never skipped** ✅. Agents report their work up to `quality_review`. The final
  `done` is a human's call.
- **Least privilege** 🚧. Fine-grained permissions decide what each agent and each person is allowed to
  touch.

## License and vision

Open source, free forever (MIT, see [`LICENSE`](./LICENSE)). A managed cloud for busier companies is on
the way. Private for now, going public when it's ready. Missing something you'd need to actually run your
business here? That's the whole idea, so
[open an issue](https://github.com/outcrop-labs/talaria/issues) and help shape it.

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
> [`ROADMAP.md`](./ROADMAP.md) and [`docs/TODO.md`](./docs/TODO.md). New here? Start
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
- **Comms** ✅. Every conversation in one place, Slack-shaped but agent-native: persistent **#channels**,
  **Relays** (named ad-hoc gatherings of people + agents around a purpose — they *conclude* with a
  summary and archive), teammate DMs, and agent DMs where every topic starts a fresh thread and idle
  chats distill into the org's retrievable memory instead of piling up scrollback forever.
- **Plan** ✅. A first-class planning surface: think through the work with an agent while a **living plan
  document** (a real artifact) takes shape side by side — the agent keeps it synced from the
  conversation — then draft dependency-aware tickets from the document onto any board, formatted on your
  org's ticket templates. Multiplayer plans are next.
- **Design and creative** 🔭. Agents and humans making creative work side by side.
- **Finance** 🔭. Agent and human finance that plugs into the big accounting and HR platforms.
- **Coding** 🔭. Agent-driven coding right in the app (probably [opencode](https://github.com/sst/opencode)
  wired into the UI).
- **Artifacts that stick around** ✅. Docs, microsites, sheets, and files with versioning, sharing, and
  public hosting — attached to tickets, plans, chats, and KB docs, exported to Google Workspace, created
  and updated by agents through the toolkit. Good work stops getting lost in a chat scroll.

## The agents, treated like first-class team members

- **Role-ready base agents** 🔭. Spin up pre-built agents made for common business roles (support, sales,
  finance, dev, ops, and so on) right from Talaria. They come already configured and ready to get to
  work, so you can hire a whole starter team in a few clicks and tweak from there.
- **Personal assistants** ✅. Everyone gets their own Hermes agent through a guided onboarding
  wizard — named, personalized, and owner-controlled (handle, model tier, skills, memory, schedules)
  with its own container, key, and private knowledge.
- **Versioned identity** ✅. Each agent's soul, config, personality, skills, and memory are version
  controlled with diff-and-restore workspaces — and Muse drafts or refines any of them from a prompt.
- **On YOUR team** ✅. Configure the organization once (Admin → Organization) and every agent anchors its
  identity to your business — in generated souls and in every rendered one. No agent introduces itself
  as belonging to a platform or model vendor.
- **Zero-downtime changes** ✅. Applying a config, MCP, or org change *rolls* the agent: a fresh container
  comes up on a new port, traffic cuts over only once it's healthy, in-flight replies drain. Edits never
  kill a conversation.
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

Underneath the app is the fleet: Hermes agent containers that Talaria renders and manages. Everything
routes through Talaria's own **gateway** — each agent calls it for LLM completions (Talaria routes to the
providers you register), and Talaria reaches each agent's persona gateway directly for chat. One
`talaria` network, published images, no Dockerfiles. You declare a fleet once, and every node stays a
full Hermes agent.

```
                     Talaria UI  (our own app)
                           │
        ┌──────────────────┴───────────────────┐
        ▼                                       ▼
  Talaria GATEWAY  /api/llm/v1          Talaria's own Postgres/Redis
  • agents' LLM → registered            (boards, tickets, teams, cost,
    providers (guarded, metered)         activity, secrets — owned not proxied)
  • chat → each agent directly                   ▲
        │                                        │
   ┌────┴────┬────────┬────────┐                 │
   ▼         ▼        ▼        ▼                  │
 agent-1  agent-2   …       agent-N ─────────────┘
   (each a full Hermes agent, on the talaria network)
     register / heartbeat / report
```

- **Run your own LLMs** 🚧. Self-hosted backends (Ollama, vLLM, llama.cpp, and friends) already register
  alongside hosted providers, serve agents through the gateway, get probed live on `/inference`, and
  split the cost view. Managing the inference containers themselves from Talaria is next.
- **Container orchestration** 🔭. A [Dokploy](https://dokploy.com)-style backend for the containers
  Talaria spins up (personal agents, services), self-hosted.
- **Analytics everywhere** 🔭. Full agent analytics plus analytics across work, chats, and projects, with
  token cost and ROI running through all of it.

### The pieces (today)

| Path | Piece | What it does |
|---|---|---|
| [`ui/`](./ui) | **Talaria app** (Vite + TanStack Start) ✅ | The product: boards, tickets, teams, multiplayer, auth, the LLM gateway. Keeps state in Postgres/Redis. |
| [`fleet/`](./fleet) | **rendered fleet** (gitignored) | Talaria-owned: one chassis renders every agent into `fleet/docker-compose.yml` + `fleet/fleet.json` (model → each agent's persona-gateway url + key). |
| [`docker/dev-compose.yml`](./docker) | **dev infra** ✅ | Compose for Postgres + Redis. The app runs on the host; the fleet runs under its own `talaria-fleet` compose project. No custom images — official/published images only. |
| [`plugin/talaria/`](./plugin/talaria) | **Talaria Hermes plugin** ✅ | Rides on each agent: registers, heartbeats for work, reports up to `quality_review` (never `done`). |

## What works today

The workspace, the fleet harness, and the money view are shipped and running:

- Shareable kanban boards (personal or team), a single Board settings modal, and a restrictive per-board
  agent policy (you opt in to which agents can touch a board).
- Rich tickets: WYSIWYG markdown description, comments, activity, watchers, a review gate, effort,
  multiple assignees, dependencies, time that adds itself up, per-ticket token spend, and links straight
  to any ticket. Drag-and-drop board plus a configurable list view; multiplayer over Redis pub/sub + SSE.
- **Comms**: channels, Relays, teammate DMs, and agent DMs in one view. @mention an agent — or
  `@agent:tier` — and the reply streams in live. Relays **conclude** (summary posted + indexed, then
  archive); idle agent chats **distill** into the activity brain and archive; every new agent topic is a
  fresh thread so context stays bounded. Membership manages inline (avatar-stack pickers in the header).
- **Plan**: plan conversations with a side-by-side living plan document the agent keeps synced; Draft
  tickets treats the document as the source of truth and proposes real dependencies you review as
  "blocked by" chips before anything is created.
- **Templates**: an org library of ticket + plan formats (markdown skeleton + agent guidance). Boards
  bind their set with a default, agents can carry overrides, and every creation surface — agent
  drafting, plan docs, even bare quick-add tickets — resolves and applies the right one automatically.
- **Organization identity**: set the business name + what it does once; muse-generated souls anchor to
  it and every rendered agent introduces itself as part of your team. Saving rolls the fleet with zero
  downtime (fresh container up + healthy before the old one retires, in-flight replies drained).
- The agent harness: design agents from a description (Muse drafts the identity, soul, and starter
  skills for review), federate outside Hermes agents in as natives, and run everything from one
  Talaria-owned chassis. Souls/models/tiers/MCP servers are immutable revertible versions with diffs;
  skills and memory edit live in versioned workspaces; per-agent secrets are encrypted and UI-managed;
  native Hermes cron schedules are created per-agent or fleet-wide — all in-app.
- Knowledge & retrieval: an Outline-style markdown drive (`/knowledge`) — a searchable, nestable tree
  of docs with drag-to-reparent, emoji icons, breadcrumbs, an auto table-of-contents, and backlinks.
  Full-text search across the base; every doc is version-controlled (view + restore any revision);
  org/private/public visibility with public share links. The shared editor does task lists, tables, and
  images, round-tripping to markdown so agents read/write it cleanly; top-level folders are themselves
  editable documents with an overview, and you can cross-link any doc with fuzzy search. Every doc syncs
  to exactly one RAG brain by its visibility: **official** org docs feed the **organization brain** all
  agents ground on; **private** docs feed the owner's **personal brain** (provisioned with their personal
  assistant, readable only by them and their agent); drafts ground nobody. A separate **workspace-activity
  brain** ambiently indexes chat/channels/tickets (plus, over time, plans/research) as a `search_knowledge`
  tool agents call on demand — index-don't-copy, permission-scoped. Admins spin up custom **departmental
  collections** and bind each to specific users or agents (Admin → Retrieval). Qdrant + a self-hosted
  TEI embedder under the hood.
- The ledger: every generation classified local vs cloud, priced automatically from a public catalog
  (override anytime), per-agent and per-ticket spend, plus `/activity`, `/alerts`, and `/inference` for
  ops. The agent MCP (`talaria-mcp`) exposes only the safe board tools.
- Guardrails: agents triage but can't assign or complete their own work.

## Run it

First time on a machine — one script sets up everything from blank (generates
secrets and an admin login, writes `ui/.env` + the fleet config plane, creates
the docker network, pulls infra images, installs deps):

```bash
./scripts/setup.sh         # prints your generated admin credentials
./scripts/dev.sh           # postgres + redis + the app → http://localhost:5273
```

Sign in with the credentials `setup.sh` printed, add an LLM endpoint on
`/models`, then design your first agent on `/agents` (Muse drafts the whole
thing from a description). Both scripts are idempotent; `dev.sh` waits for
Postgres before booting the app (avoids the cached-migration-failure gotcha).
Dev state runs in containers (`talaria-postgres-dev` on :5544,
`talaria-redis-dev` on :6399 — override with `TALARIA_PG_PORT` /
`TALARIA_REDIS_PORT`). Agents run under the separate `talaria-fleet` compose
project, rendered into the gitignored `fleet/` dir from one Talaria-owned
chassis (`fleet/chassis.yml`; agent image via `HERMES_IMAGE`). More detail in
[`ui/README.md`](./ui/README.md) and [`HANDOFF.md`](./HANDOFF.md).

The fleet engine: you don't wire it by hand. Design an agent in the app
(`/agents`), and Talaria renders `fleet/docker-compose.yml` + `fleet/fleet.json`
from one chassis and brings the agents up under the `talaria-fleet` compose
project — all on the single `talaria` docker network, all published images, no
Dockerfiles. Each agent's LLM and persona chat route through Talaria's own
gateway; the app reaches each agent directly on its published port.

Agents themselves are designed/federated in the app and rendered into `fleet/`
automatically, including the routing manifest (`fleet/fleet.json`) the app uses
to reach each agent directly.

## Safe by design

Talaria does manage your agents, that's the point, but it does it carefully, in the open, and in a way
you can always undo.

- **Every agent is a real Hermes agent** ✅. Chat and streaming go to each agent's real gateway with its
  own key. No weak "worker" tier.
- **Version-controlled internals** ✅. An agent's soul, memory, skills, tools, and MCP servers are
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

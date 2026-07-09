# Talaria (design notes)

> *Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

> ⚠️ **Work in progress, not production ready.** This is an internal design and rationale doc. For the
> current product vision, positioning, and feature status (with ✅/🚧/🔭 markers), [`README.md`](./README.md)
> is the canonical source of truth. The product itself, the app in [`ui/`](./ui), is where the active work
> lives; read this doc for the design thinking behind the fleet engine underneath it. Parts of this doc
> describe Phase-1 scaffolding that is now legacy, clearly marked as such below.

## What Talaria is

Talaria is the nerve center for a lean, agent-powered business: one multiplayer workspace where your people
and your AI agents share the same surfaces (boards, chats, plans, design, finance, code) and run the company
together in real time, with sensible human-in-the-loop guardrails on the calls that matter. Every agent is a
full [Hermes](https://github.com/outsourc-e/hermes-workspace) agent (memory, skills, run loop, all intact);
other runtimes come later.

Talaria has its own UI and owns its own state. We build the app by lifting the good parts out of
hermes-workspace (mostly the chat and agent UX) into our own code, and we lifted mission-control's
capabilities (task queue, cost, activity) into our own Postgres and Redis instead of proxying a running
copy. We do not run hermes-workspace and we do not front a live mission-control; those are parts bins we
pulled from, not services we operate. We manage each agent's internals (soul, memory, skills, tools, MCP
servers) in-app and version controlled, and the guardrails keep agents to create/triage work (they can't
self-assign or self-complete). See the README for the full feature list and status markers.

The rest of this doc is about the **fleet engine**: the runtime underneath the app that lets Talaria talk to
a whole fleet of Hermes agents.

## The fleet engine (current)

Underneath the app is the fleet runtime. The current shape is the **gateway plane**: a fleet multiplexer
(port 8642) that exposes your whole fleet through one endpoint so Talaria can reach every agent uniformly.

- `/v1/models` lists the whole fleet (each agent shows up as a model).
- `/v1/chat` is routed by model to the right agent, with a per-agent key, streamed over SSE.

You declare a fleet once (a manifest that maps each model to a gateway url and key), and every node stays a
full Hermes agent. A small Hermes plugin rides on each agent: it registers, heartbeats for work, and reports
its results up to `quality_review` (never straight to `done`, so a human signs off). Talaria keeps its own
state (boards, tickets, teams, cost, activity) in its own Postgres and Redis, owned rather than proxied.

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

This is the piece that is shipped and running today, alongside the product surfaces — the PM suite, Comms,
the multiplayer Plan view, Knowledge/Artifacts, versioned agent internals, personal assistants, and auth.
See [`README.md`](./README.md) and [`ROADMAP.md`](./ROADMAP.md) for current status; finance, design, and
in-app coding are still ahead of us.

---

## Legacy: Phase-1 fleet-engine scaffolding

> 🕰️ **This whole section is legacy.** When the project started, it was scoped as a small bridge that let
> two external tools work together: **hermes-workspace** (a Hermes-native UI) driving **mission-control** (an
> agent-agnostic fleet manager) as its "brain," with Talaria as a plugin/shim in between. That is **not**
> Talaria's identity or current architecture anymore. We lifted those capabilities into our own app and our
> own Postgres/Redis (see above). The "dashboard plane" proxy, the mission-control bridge, the conductor
> mission routing, and the mission-control adapter described below were Phase-1 scaffolding for fronting
> those external UIs. We keep the notes for technical reference and history, but read them as the design of a
> superseded bridge, not the product.

### (Legacy) TL;DR

The original plan was to build **Talaria**, an MIT-licensed bridge (shipped as an installable **Hermes
plugin**) that let the Hermes-native **hermes-workspace** UI use **mission-control** as its fleet "brain."
Integration was meant to be **config-only on the workspace side** (point `HERMES_DASHBOARD_URL` at Talaria);
Talaria would transparently proxy the dashboard (:9119) and intercept *only* mission-dispatch calls,
translating them to mission-control's REST API. The Hermes agent runtime (:8642) was **never touched**, so
native behavior was preserved and the whole thing was **opt-in and instantly reversible**.

### (Legacy) Thesis

Keep the two best-of-breed tools unmodified. Insert one small bridge service that speaks hermes-workspace's
dashboard-mission contract on one side and mission-control's REST API on the other. Ship it as an
installable Hermes plugin so the whole thing is `hermes plugins install <you>/talaria` plus one env var. No
forks required to *use* it. The payoff was the Hermes-ecosystem-focused UI backed by a more proven
open-source fleet manager as the orchestration brain. (Talaria today keeps the UX we liked but owns the
whole surface itself, rather than bridging to those external tools.)

### (Legacy) Orchestration advantages (the original "why")

Native hermes-workspace gave smart mission *decomposition* (Conductor) but only **local, ephemeral
execution** (native-swarm = tmux workers on one box). mission-control added a mature **execution and fleet**
layer Hermes lacked natively. The bridge wired decomposition into durable, distributed execution. What that
was meant to gain over native:

1. **Cross-host fan-out**: a real task queue plus per-agent heartbeat polling
   (`/api/agents/{id}/heartbeat`) across agents on *different machines*, vs one box's tmux workers.
2. **Durable task state**: SQLite-persisted, surviving a workspace/UI restart (native swarm is ephemeral).
3. **Fleet-wide cost/token governance**: Hermes had **no** native fleet cost rollup.
4. **Health monitoring and crash recovery**: heartbeats plus task reassignment, filling #344's documented
   "no crash recovery or health monitoring" gap.
5. **Inter-agent messaging**: mission-control exposed `/api/agents/message` plus `/api/agents/comms`
   (verified), a coordination channel Hermes lacked natively (#344: agents "can't talk to each other").
6. **Heterogeneous fleets**: the adapter layer also spoke CrewAI / LangGraph / AutoGen / Claude SDK, so
   Hermes and non-Hermes agents shared one board.
7. **Decoupled dispatch**: the REST brain was drivable from CLI / cron (scheduler) / CI / webhooks, not just
   the UI. The Hermes-native UI became one client of the fleet, not the single control point.
8. **Governance and multi-operator**: API-key auth, RBAC, workspace isolation (`/api/super/*`).

### (Legacy) It did NOT back away from Hermes's strong suits, it depended on them

The bridge's non-destructive design (never touching the gateway :8642) meant **every node in the fleet
stayed a full Hermes agent**: persistent memory, learning loop, skills, "grows with you." It orchestrated
*smart* nodes, not thin workers.

On the #344 "agent cognition gap," to be precise, it had two halves, sourced from two places:

- **Intra-agent cognition** (one agent's memory / learning / skills / reasoning depth): filled by **Hermes
  itself**, and **preserved 100%** by the bridge. This was the half Hermes's strong suits cover, and the
  reason non-destructiveness was non-negotiable.
- **Inter-agent cognition** (agents talking and sharing state): Hermes couldn't do this natively. The bridge
  got a meaningful chunk from **mission-control's `/api/agents/message` plus `/api/agents/comms`**, a
  task-level coordination fabric.

So the combination was **smart Hermes nodes (intra-agent) plus a messaging/coordination fabric (inter-agent)
= orchestrated multi-agent cognition.** The one genuinely-remaining gap was a true **shared memory pool**
(shared in-context reasoning state, #344's deepest phase): messaging *approximates* it but isn't a single
shared brain. The forward path noted at the time: Hermes plugins can register **memory backends** (verified),
so a future Talaria could ship a shared-memory backend *as a Hermes plugin* and close even that.

### (Legacy) Why this was the right shape (the key findings)

- **hermes-workspace had no UI plugin system, but didn't need one.** Its Conductor reached the mission
  backend through a single env var: **`HERMES_DASHBOARD_URL`** (default `http://127.0.0.1:9119`). Point it at
  Talaria and the workspace talked to us with **zero code changes on its side.** This was the linchpin.
- **mission-control had no plugin SDK either**, but exposed exactly the surfaces needed: a documented REST
  API (OpenAPI 3.1.0, title "Mission Control API") to drive, `/api/agents` (POST register, GET list),
  `/api/agents/{id}/heartbeat` (GET = poll for work), `/api/tasks`, plus `/api/agents/message` and
  `/api/agents/comms` for inter-agent messaging, and an adapter folder (`src/lib/adapters/`) where a
  **Hermes adapter was a clean upstream PR**. MIT-licensed. *(Verified against `openapi.json`; earlier
  `/api/agents/register` plus `/api/tasks/queue` names were search-summary approximations, the real shape is
  `/api/agents` plus heartbeat-poll.)*
- **Hermes itself *did* have a real plugin system**: `~/.hermes/plugins/`, a `plugin.yaml` manifest,
  `hermes plugins install owner/repo`, and an allow-list security model. **That is where Talaria's "plugin"
  actually lived and how people installed it.**

### (Legacy) Architecture (the seam)

```
hermes-workspace UI
   │  Conductor → HERMES_DASHBOARD_URL  (point this at Talaria)
   ▼
┌─────────────────────────────────────────────┐
│  TALARIA  (our code, MIT)                    │
│  • intercepts mission-dispatch calls         │
│  • translates → mission-control REST schema  │
│  • reverse-proxies all OTHER dashboard       │
│    endpoints (sessions/skills/config/MCP)    │
│    straight through to the real :9119        │
└─────────────────────────────────────────────┘
   │  REST: /api/agents, /api/agents/{id}/heartbeat, /api/tasks, /api/agents/message
   ▼
mission-control  ──►  N deployed Hermes agents (registered via adapter)
   (fleet dispatch · cost · RBAC · telemetry)
```

**The catch this had to handle:** `HERMES_DASHBOARD_URL` redirected the **whole** dashboard (port 9119 also
serves sessions/skills/config/MCP), not just mission dispatch. So the bridge had to **reverse-proxy the
non-mission endpoints untouched** and only intercept the mission ones. (The upstream PR below gave a cleaner
long-term path.)

### (Legacy) Non-destructiveness, preserving Hermes native functionality (was a hard requirement)

The bridge was never to break Hermes's native behavior. The blast radius was deliberately tiny.

**What the bridge did NOT touch:**
- **The Hermes agent runtime** (gateway, port **8642**): chat, model streaming, jobs, plus the agent's run
  loop, memory, skill *execution*, plugins, `delegate_task`/native subagents. The bridge was nowhere near
  it.
- **The core workspace chat/streaming experience** also went to the gateway (8642), **not** through the
  bridge.

**What the bridge DID sit in front of:** only the dashboard service (port **9119**), session/skills/config/MCP
browsing (passed through untouched) and mission dispatch (intercepted/translated).

**Design rules that made this safe:**
1. **Transparent pass-through by default; allowlist-intercept.** Forward all 9119 traffic byte-for-byte
   (headers, auth, SSE/streaming, websockets); only rewrite the specific mission-dispatch routes.
   Unknown/new endpoints, including ones future Hermes versions add, pass straight through. (A denylist would
   be fragile; use an allowlist.)
2. **Opt-in and instantly reversible.** Active only because `HERMES_DASHBOARD_URL` points at Talaria. Unset
   it and 100% native behavior is restored (incl. native-swarm dispatch). **The bridge never edited Hermes's
   files, config, or the dashboard**, so no residue.
3. **One intentional substitution, clearly bounded.** When active, the bridge replaced hermes-workspace's
   native-swarm mission dispatch (tmux workers) with mission-control dispatch. It swapped the *fleet dispatch
   layer*, never the *agent runtime*. The agent executed tasks exactly as before; only who handed it the task
   changed.
4. **Preferred mode was the `HERMES_MISSION_API_URL` upstream PR** (see below): with it, the dashboard talked
   directly to native :9119 and the bridge only answered mission calls, removing the proxy risk surface
   entirely. Full-dashboard-proxy was the fallback for un-patched workspaces.

**Caveat / M0 dependency:** the safety guarantee rested on the allowlist being correct, which meant
**verifying the exact 9119 route boundaries (mission vs. native) was an explicit M0/M1 deliverable.**

### (Legacy) What it was to ship (three artifacts, increasing ambition)

1. **The bridge service**, the core deliverable. Standalone (Node/TS to match both projects' stack). Config:
   `HERMES_DASHBOARD_URL` in plus mission-control URL/API key out. MIT.
2. **A Hermes plugin wrapper**: `plugin.yaml` manifest so it installed via `hermes plugins install
   <you>/talaria`, respected the allow-list, and registered a tool/hook to launch the bridge. This was the
   "plugin for open-source release."
3. **A mission-control Hermes adapter (upstream PR)**: `src/lib/adapters/hermes/` so each Hermes agent's
   register/heartbeat/task-report was first-class in the proven brain.

### (Legacy) Recommended upstream contribution (de-risked everything)

Send hermes-workspace a tiny PR adding a **`HERMES_MISSION_API_URL`** override that decoupled mission
dispatch from the dashboard URL. That turned their "conditional fallback" into a true pluggable interface,
then the bridge no longer had to proxy the whole dashboard, just answer mission calls. Good for the
ecosystem, and it made the integration officially sanctioned rather than a redirect hack.

### (Legacy) Milestones

- **M0, Spike (½ to 1 wk):** (a) Capture hermes-workspace's actual mission-dispatch request/response
  payloads (one task / decomposed mission / broadcast) and map field-by-field to mission-control's
  `/api/tasks` schema. (b) Enumerate the 9119 routes and classify each as **mission (intercept)** vs **native
  (pass-through)**, this list *is* the safety allowlist. Output: a contract diff plus the intercept allowlist.
  Go/no-go, tells us whether the shim is a weekend or a month.
- **M1, Pass-through proxy:** the bridge proxies 9119 transparently (headers/auth/SSE/websockets
  byte-for-byte); workspace works **identically** through it with nothing intercepted yet. Proves native
  functionality is preserved before we translate anything.
- **M2, Mission translation:** intercept and translate single-task dispatch into mission-control; tasks
  appear on the fleet board and route to a real agent.
- **M3, Decomposition plus broadcast plus status round-trip:** full Conductor parity; cost/telemetry visible
  in mission-control, status reflected back in the workspace.
- **M4, Package as Hermes plugin** plus the mission-control adapter PR.
- **M5, OSS release:** repo, MIT license, README with the architecture diagram, `docker compose` demo wiring
  workspace plus Talaria plus mission-control plus 2 mock Hermes agents, version-pin compatibility matrix.

### (Legacy) Open-source release specifics

- **License:** MIT (matches both deps, no copyleft friction). *(Talaria is still MIT, free forever, see the
  README.)*
- **Versioning:** publish a **compatibility matrix** (Talaria vX ↔ hermes-workspace vY ↔ mission-control vZ),
  both moved fast, so this was non-negotiable.
- **Positioning:** "Use the Hermes-native UI you already like with a proven, agent-agnostic fleet manager as
  the brain." (This positioning is superseded; see the README for Talaria's current positioning.)

### (Legacy) Risks that were sized at M0

- **Schema drift** across two fast-moving APIs, mitigated by the compat matrix plus the upstream
  `HERMES_MISSION_API_URL` PR.
- **Two sources of truth** (both tools tracked cost plus task state): decide early that **mission-control
  owns the task queue and cost ledger**; the workspace is a view.
- **Cross-host queue semantics:** confirm mission-control dispatches cleanly to agents on *different* hosts
  (its gateway-optional standalone mode suggested yes, verify in M2).

### (Legacy) Reference projects

| Project | Role | Notes |
|---|---|---|
| `outsourc-e/hermes-workspace` (~5.9k★, MIT) | **UI** | Hermes-native; Conductor plus `HERMES_DASHBOARD_URL` seam |
| `builderz-labs/mission-control` (~5.5k★, MIT) | **Brain** | Agent-agnostic fleet manager; REST API plus adapter layer |
| `NousResearch/hermes-agent` | **Runtime** | Real plugin system (`~/.hermes/plugins/`, `plugin.yaml`) |

### (Legacy) Verification status (primary-source checked)

Confirmed against repos/docs (quotes, not inference): the **8642 gateway / 9119 dashboard** port split;
`HERMES_DASHBOARD_URL=http://127.0.0.1:9119` plus `HERMES_API_URL=http://127.0.0.1:8642`; Conductor's
"dashboard mission API → `mode: native-swarm`" fallback; the Hermes plugin system (`hermes plugins install
user/repo`, `plugin.yaml`, `~/.hermes/plugins/`, allow-list, disabled by default, can register
tools/hooks/**memory backends**); mission-control's OpenAPI 3.1 surface incl. `/api/agents`,
`/api/agents/{id}/heartbeat`, `/api/tasks`, `/api/agents/message`, `/api/agents/comms`. **Residual risk:**
exact request/response *bodies* were still M0 source-reading work (web fetches summarize). The safety
guarantee rested on the 9119 intercept allowlist, verified in M0/M1.

### (Legacy) Open questions

1. Where does the bridge run, a sidecar next to each agent host, or one central instance? (Decide with
   cross-host findings at M2.)
2. Do we maintain the mission-control Hermes adapter as an upstream PR, or vendor it until merged?

### (Legacy) Handoff notes

**Decisions locked at the time:** name = **Talaria**; license = **MIT**; stack = **Node/TS** (matches both
deps); **mission-control owns the task queue plus cost ledger**, the workspace is a view; **preferred
integration mode** = the `HERMES_MISSION_API_URL` upstream PR, with full-dashboard-proxy as the fallback for
un-patched workspaces.

**Reference repos:** `outsourc-e/hermes-workspace` (UI), `builderz-labs/mission-control` (brain),
`NousResearch/hermes-agent` (runtime plus plugin host).

**Definition of done for M0** (the first task at the time): (1) a field-by-field map of hermes-workspace's
three Conductor dispatch shapes, single task / decomposed mission / broadcast, onto mission-control
`/api/tasks`; (2) the **9119 route allowlist** (mission = intercept vs native = pass-through), which *is* the
safety contract; (3) a weekend-vs-month effort call. All three read from source, not summaries.

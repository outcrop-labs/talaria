# Phase 2: Talaria's own UI

> Status: **in progress (updated 2026-07-02).** The app in [`ui/`](../ui) (Vite + TanStack Start) has
> the **Mercury** design system (dark + light), pluggable auth (Google OAuth + username/password, each
> independently toggleable, Redis-backed sessions), durable state on Postgres + Redis, and a full
> **project-management suite** owned in Talaria's own stack. The old Phase 1 scaffolding (the dashboard
> plane + mission-control bridge that once fronted a running hermes-workspace + mission-control) is
> legacy now; the gateway plane is the current fleet engine.
>
> **Built so far (Phase 2):**
> - **Boards & teams:** shareable kanban boards (personal/team), restrictive board-scoped agent policy
>   (allow-all opt-in), consolidated Board settings modal, archive + delete, teams + members.
> - **Tickets:** rich WYSIWYG (TipTap → markdown) description with read/edit + slide-in full editor,
>   comments (Ctrl+Enter), activity tab, watchers, quality-review gate. **Directly-linkable routes**
>   (`/boards/:boardId/:taskId`) + copy-link on cards/rows/modal.
> - **Fields:** priority, effort (XS to XL), multiple assignees (board-scoped), labels, due date,
>   dependencies (blocked-by/blocks), auto-accumulated time-spent. Estimates removed (silly for agents).
> - **Statuses:** added a **Blocked** column; drag-and-drop board + list view with configurable,
>   drag-reorderable, click-to-sort columns (persisted per board).
> - **Multiplayer:** live boards via Redis pub/sub → SSE.
> - **Agent guardrails:** agents can create/triage but can't self-assign or self-complete.
>
> **Next:** the **agent MCP** (`talaria-mcp`) exposing only the safe create/triage tools; then chat
> (agent picker + streaming over the gateway plane), notifications/@mentions, cost/token ledger, and
> the admin console. Token-spend + per-LLM-API attribution per ticket is a tracked follow-up.

## The idea

Talaria is its **own** app now, not a seam between someone else's UIs. This doc plans the chat + ops
half of it: we lift the good parts out of hermes-workspace and mission-control (both MIT, both just parts
bins) and wire them into Talaria's own app, on Talaria's own backend. Two faces over that one backend:

- **Simple view (for everyone):** the friendly cockpit. Pick an agent, chat, kick off a mission, watch it
  run. No ops chrome, nothing scary.
- **Advanced view (for fleet maintainers):** the full ops console. Fleet health, cost/token governance,
  the task board, agent roster, telemetry, RBAC.

One login, one design language, one release, all on Talaria's own state.

## Principles

1. **Not a fork.** Forking both upstreams is a maintenance trap. Instead, build our own app and **lift in
   the components we need** from each (both are MIT, so we can lift freely) and drop the rest.
2. **Talaria owns its own state, it doesn't proxy anything.** The gateway plane (`/v1/models`,
   model-routed chat, merged sessions) is Talaria's own runtime and the current fleet engine. The ops
   functionality (agent registry, heartbeat, task queue, cost/token ledger, activities, alerts) is
   **lifted from mission-control into Talaria's own stack** (Postgres/Redis) rather than proxied from a
   running mission-control, so there's no upstream service dependency and no upstream to track. Agents
   report to **Talaria**. (Both hermes-workspace and mission-control are lift sources, not backends.)
3. **Two views, one app.** A single frontend with a mode toggle (or role-gated: most people land on
   simple, maintainers can flip to advanced). Not two separate apps.
4. **Incremental.** Ship the simple view first (chat + agent switcher), then layer the advanced view. Each
   is independently useful.
5. **Own the experience.** One design system, our branding, no upstream to track.

## Architecture

```
                 Talaria UI  (our app, one identity)
        ┌──────────────────────────────────────────────┐
        │  simple view (normies) │ advanced view (ops)  │
        └───────────────────────┬──────────────────────┘
                                │ everything via Talaria's own BFF
                                ▼
                 Talaria server (our brain)
        ┌──────────────────────────────────────────────┐
        │ gateway plane        │  owned ops core         │
        │ (/v1/models, chat,   │  (agents, tasks, cost,  │
        │  merged sessions)    │  activities, Postgres)  │
        └──────────┬───────────┴───────────┬────────────┘
                   ▼                        ▲
             the agent fleet ──────────────┘ register / heartbeat / report → Talaria
```

The UI talks to **one backend: Talaria's own BFF**. Chat/agents ride the gateway plane; ops data is
served from Talaria's own Postgres (usage today; a full ripped-in registry/task-queue/ledger as it
lands). mission-control is a **component-lift source**, not a runtime dependency.

## Tech

- **Framework: Vite + TanStack (Start / Router). DECIDED.** React + TypeScript. This matches
  hermes-workspace, so the harder-to-port half (streaming chat, the agent/model switcher, the session
  sidebar, the conductor UI) lifts with minimal friction, and Vite gives fast dev. The trade: mission-
  control's ops/data views are Next.js and get ported into TanStack (data fetching + routing) rather than
  lifted as-is. That's the cheaper direction overall, since data-fetching/table/board views port more
  easily than live streaming chat. Practically: TanStack Start for the app + server routes (same shape as
  hermes-workspace's own API routes), TanStack Query for data, TanStack Router for the shell.
- **Design system: Mercury. DECIDED + BUILT.** A hand-rolled Tailwind v4 token system in the same
  cyberpunk-HUD family as hermes-workspace (deep-navy sci-fi, `framer-motion`), deliberately *not* a
  component library, because both upstreams already are hand-rolled sci-fi (not shadcn look-alikes). We
  match hermes-workspace's `--theme-*` token *contract* exactly so its chat components lift with near-zero
  friction, but ship Talaria's own identity: Mercury-the-planet neutrals (graphite / basalt / regolith)
  with a violet-to-magenta neon accent. Two modes, `mercury` (dark) and `mercury-light`. Tokens live in
  [`ui/src/styles.css`](../ui/src/styles.css).
- **Auth/SSO: pluggable + env-gated. Google OAuth + username/password shipped.** Each provider is
  independently enable-able (enabled only when its flag is on *and* its secrets are present), via the
  registry in [`ui/src/server/auth/config.ts`](../ui/src/server/auth/config.ts). Sessions are stateless
  HMAC-signed cookies. Adding GitHub / Microsoft / generic OIDC is a small change. Role-drives-view and
  the Cloudflare Access gate still layer on top.
- **Streaming:** the chat view needs SSE (already how the gateway plane streams).

## What to lift from each upstream (candidates)

Early task: a **component inventory** pass to mark each as lift-as-is / adapt / rebuild.

**From hermes-workspace (the chat/agent UX):**
- Chat thread + message rendering, streaming handling
- Model/agent switcher (becomes the agent picker over `/v1/models`)
- Session sidebar (over the merged `/api/sessions`)
- Conductor mission UI (create / poll / cancel)
- Optionally the swarm/kanban board view

**From mission-control (the ops/observability UX):**
- Agent roster with status + task stats + cost per agent
- Task board (kanban) + task detail
- Cost / token governance views (`tokens/by-agent`)
- Activity feed, standup, system-monitor, workload, alerts
- Workspace / RBAC / settings surfaces

## The two views

**Simple (everyone) the default landing:**
- Agent picker (from `/v1/models`)
- Chat with the selected agent (streaming, its memory/skills intact)
- "Start a mission" + a simple status card
- Maybe a lightweight "my recent conversations" (from merged sessions)
- Everything else hidden

**Advanced (maintainers) one toggle away:**
- Fleet dashboard: agents online/idle/busy, per-agent cost/tokens, health
- Task board (the full MC kanban) + task detail
- Missions, sessions across the fleet, activity feed, alerts
- Cost governance, RBAC, config

## Milestones

- **P2.0 Component inventory + shell decision. ✅ (core done).** Framework (Vite + TanStack Start) and
  design system (Mercury) decided and built; the `ui/` shell, login, and auth are live. Both lift sources
  are vendored under [`vendor/`](../vendor) (gitignored). Still open: the per-component
  lift/adapt/rebuild map (weighted toward mission-control's Next-to-TanStack ports).
- **P2.1 Simple view MVP. 🚧 in progress.** Shell + branding + auth done; the agent MCP
  (`talaria-mcp`, [`mcp/`](../mcp)) shipped with safe create/triage tools only; **next up** is the agent
  picker + streaming chat over the gateway plane (`/v1/models`, model-routed chat). This is Talaria's own
  chat surface, built by lifting the chat/agent UX out of hermes-workspace.
- **P2.2 Advanced view MVP.** Fleet dashboard (agents + cost) + task board, served from Talaria's own
  Postgres, behind the mode toggle.
- **P2.3 Missions + sessions + activity** in both views; polish the simple-to-advanced handoff.
- **P2.4 Identity + release.** Branding, docs, one deployable image. Talaria is the whole front door on
  its own state (the old dashboard-plane / mission-control-bridge scaffolding stays legacy).
- **P2.5 (later) All-in-one self-hosted super-dashboard.** Beyond the Hermes fleet, monitor local
  **inference stacks** (Ollama, vLLM, llama.cpp, LM Studio, TGI, and friends): health, loaded models,
  GPU/VRAM, tokens/sec. Turns Talaria into the single pane of glass for a self-hosted Hermes rig, the
  agents *and* the metal underneath them.

## Open questions

- **BFF or direct?** Does the UI hit the gateway plane + MC REST directly, or does Talaria add a
  backend-for-frontend that unifies them (auth, shaping, one origin)? Leaning BFF for a clean single-origin
  app, but it's more to build.
- ~~Framework~~ **Decided: Vite + TanStack** (see Tech). Implication for P2.0: mission-control's ops views
  are the ones needing a port (Next → TanStack), so weight the component inventory toward them.
- **How much to lift vs rebuild.** Lifting keeps parity fast but inherits upstream quirks; rebuilding is
  cleaner but slower. Decide per component in P2.0. (hermes-workspace components lift; mission-control
  views mostly rebuild-in-TanStack.)
- **Where the simple ↔ advanced toggle lives**, and whether it's user-choice or purely role-gated.
- ~~Design system / branding~~ **Decided: Mercury**, hand-rolled Tailwind v4 tokens (dark + light),
  violet→magenta on Mercury-planet neutrals; matches hermes-workspace's token contract so its chat
  components lift. See Tech.

## Definition of done (first real cut)

A single Talaria UI, our branding, that anyone can open, pick any fleet agent, and chat with (P2.1),
and that a maintainer can flip to an ops view showing the live fleet + task board + cost (P2.2). At that
point Talaria is the one front door for the fleet, running on its own state, with the old Phase 1
scaffolding kept only as legacy.

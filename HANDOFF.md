# Talaria - handoff for the next agent

_Last updated: 2026-07-02. Scope: Phase 2 (Talaria's own UI). This file is a fast
on-ramp; the authoritative docs are [`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md),
[`ROADMAP.md`](./ROADMAP.md), [`CHANGELOG.md`](./CHANGELOG.md), and [`ui/README.md`](./ui/README.md)._

## What Talaria is

Talaria is the nerve center for a lean, agent-powered business: one multiplayer
workspace where your people and your AI agents share the same surfaces (boards,
chats, plans, design, finance, code) and run the company together, with human-in-the-loop
guardrails. It's Talaria's **own** app in [`ui/`](./ui) (Vite + TanStack Start,
React 19 + TS, Tailwind v4 "Mercury" design system), backed by Talaria's **own**
Postgres/Redis. Every agent is a full Hermes agent underneath.

We build the app by ripping the good parts out of hermes-workspace (chat, agent UX)
into our own UI, and we lifted mission-control's capabilities (task queue, cost,
activity) straight into our stack. We do **not** run hermes-workspace and we do **not**
proxy mission-control. Talaria owns its own state.

Underneath the app is the fleet engine, the runtime that talks to your agents. The
current fleet engine is the **gateway plane** (fleet multiplexer on `:8642`). The
older mission-control bridge / conductor bits are **legacy Phase-1 scaffolding**, kept
around but not part of Talaria's current identity or architecture.

Heads up: Talaria is a work in progress, not production ready. Only the PM suite, the
fleet engine (gateway plane), and auth ship today; everything else is on the way.

## Current state (what's built in Phase 2)

Full project-management suite, all live in `ui/`:

- **Boards & teams** - shareable kanban boards (personal/team), Board settings modal
  (General / People / Agents), restrictive board-scoped agent policy, archive + delete.
- **Tickets** - TipTap WYSIWYG description (markdown under the hood) with read/edit +
  slide-in full editor, comments (Ctrl+Enter), Activity tab, watchers, quality-review
  gate. Directly-linkable routes `/boards/:boardId/:taskId` + copy-link everywhere.
- **Fields** - priority, effort (XS-XL), multiple assignees (board-scoped agents only),
  dependencies (blocked-by/blocks), labels, due date, auto-accumulated time-spent.
  (Manual hour estimates were removed.)
- **Statuses** - Inbox · Assigned · In progress · **Blocked** · Quality review · Done
  (+ Failed / Cancelled). Drag-and-drop.
- **List view** - configurable, drag-reorderable, click-to-sort columns (persisted per
  board in `localStorage`).
- **Multiplayer** - Redis pub/sub → SSE (`/api/boards/:id/events`).
- **Agent guardrails** - agents can create/triage but **cannot** self-assign
  (`assigned` → 403) or self-complete (`done` → `quality_review`), and can't change
  assignees.
- **Agent MCP (`talaria-mcp`)** - MCP server in [`mcp/`](./mcp) (stdio, TS, built with
  `npm run build`) exposing only the safe tools (`list_boards`, `list_tickets`,
  `get_ticket`, `create_ticket`, `triage_ticket`, `comment`, `report_outcome`,
  `add_time`, `add_dependency`); no assign/complete tools. Backed by agent-key HTTP
  paths on boards/tasks/comments/dependencies: `TALARIA_AGENT_KEY` + a new
  `x-agent-name` header, board agent policy enforced everywhere, activity attributed
  to the named agent. Create lands in `inbox`, always unassigned. Unnamed key callers
  (legacy plugin heartbeat/report) keep their old access on `PUT /api/tasks/:id`.
  See [`mcp/README.md`](./mcp/README.md) for client config.
- **Chat (1:1)** - the home surface: agent picker over the gateway plane's
  `/v1/models`, durable server-owned conversations in Postgres, streamed replies
  that survive a reload (teed persist).
- **Group chat (channels)** - Slack-style channels (`/channels`) where teammates and
  fleet agents are members. Tables `channels` / `channel_members` / `channel_agents` /
  `channel_messages` (per-channel `msg_seq` counter — many concurrent writers).
  Agents reply when **@mentioned** (label or model id, `server/channel-replies.ts`):
  transcript → gateway `proxyChat`, streamed into the channel row-by-flush and
  published over `channel:<id>` pub/sub → SSE. Composer autocompletes mentions.
  Adding an agent to a channel requires the adder's access to that agent.

- **UI conventions** - one control-size scale (`sm` h-9 / `md` h-11) via `size` props
  on Button/Input/Select/Combobox (`ui/control.ts`) — never hand-set `h-*` on a
  control. **Card density is owned by `Panel`** (default `p-6`; override like
  `p-0` only for flush tables). Card internals: header block `mb-4`, tiny
  uppercase labels `mb-2`, list rows `py-3`, chip/meta clusters `mt-2.5`. Pages:
  `p-8` + `space-y-8`. People are added through `UserPicker` (combobox over `/api/users`),
  agents through the multi `Combobox`, ticket labels through `LabelPicker`
  (chips + combobox with `allowCreate`). Users set display names in Settings
  (`PUT /api/profile`); prefer `name ?? email` when rendering people.

- **Notifications** - user @mentions in channels land in the **Inbox** (`/inbox`,
  unread badge in the nav; 30s poll). `server/notifications.ts` +
  `GET/PUT /api/notifications`; the composer autocompletes members and agents.
  Mention tokens: email localpart / dashed name / first name
  (`userMentionTokens` in `server/channel-replies.ts` — the composer mirrors it).
- **Token ledger** - `usage_events` (one row per agent generation), recorded from
  both persist paths (`chat-persist.ts`, `channel-replies.ts`). Real counts via
  `stream_options.include_usage` (the gateway plane honours it — prompt tokens run
  ~17-35k/turn because each turn carries the agent's full context); char/4
  estimates flagged when absent. `GET /api/cost` + the `/cost` page (tiles, 14-day
  strip, per-agent). Dollar cost needs per-LLM pricing attribution (next-up #2).

- **Admin console** - `/admin`: user roles (pinned admins via `AUTH_ADMIN_EMAILS`,
  no self-demotion) + per-person agent allow-lists (`user_agent_access`; empty =
  all agents). `GET/PUT /api/admin/users`, admin-gated.

- **Agent harness (phase A)** - Talaria as the fleet's source of truth. Tables
  `llm_endpoints` (class local|cloud, feeds the cost split), `agent_defs`,
  `agent_versions` (immutable payloads: soul + config jsonb incl. `raw` full
  config.yaml). Importer `server/fleet-import.ts` ingests `TALARIA_STACK_DIR`
  (default `~/packledger-services/ai/orchestration`): `agents.yaml` roster +
  per-department `config.yaml`/`SOUL.md`; idempotent (canonical-JSON compare —
  jsonb reorders keys). Admin API `GET/POST /api/fleet/defs`,
  `GET /api/fleet/defs/:id/versions`; Definitions panel on `/agents`.
  Key context: Hermes `model_aliases` = one container serving all model tiers
  (that + `fallback_providers` replace per-tier scaffolding); the external
  stack's pain is 6 hand-edited places per agent — the renderer (phase B)
  generates all of them from the version payload.

- **Agent harness (phase B)** - the renderer + orchestrator. `server/fleet-render.ts`
  materializes managed versions into gitignored `fleet/` (config.yaml as **YAML
  1.1** — PyYAML semantics; generated compose derived from the source stack's
  resolved service block: external volumes `ai_hermes-<dept>`, external network
  `ai_default`, no build/depends_on/host-ports) and writes the gateway manifest
  to `stack/fleet.json`, which the bridge **hot-reloads** (watchFleet, 2s poll).
  `server/fleet-docker.ts` drives `docker compose -p talaria-fleet` (⚠️ `-p` is
  mandatory — the stack .env sets `COMPOSE_PROJECT_NAME=ai` and would hijack the
  project, orphaning the whole legacy stack). Lifecycle API
  `POST /api/fleet/agents/:id/control` (migrate/up/stop/legacy-*), containers
  API, and live status + buttons on `/agents`. **Pilot done: sam-support is
  Talaria-managed** (`talaria-fleet-agent-support-1`), memories intact; the
  legacy compose has `agent-support` behind a `retired-migrated-to-talaria`
  profile + its depends_on entries commented (prewarm, openwebui).

- **Agent harness (phase C1)** - in-app config control + the cost split.
  `POST /api/fleet/defs/:id/edit` saves soul/main/aliases/fallbacks as a new
  version (`applyConfigEdits` merges structured edits into the raw config,
  preserving extra keys when the endpoint is unchanged); `apply: true`
  re-renders + `docker compose restart`s the managed container. Reverts append
  (`POST .../versions {revertTo}`). Editor modal on `/agents`. Ledger:
  `usage_events.endpoint_class`/`llm_model` stamped per generation from the
  agent's current MAIN endpoint (60s cache in `usage.ts`); `/cost` shows the
  local/cloud share bar, stacked daily strip, per-agent "% local".

- **Agent harness (phase C2)** - create/retire from the UI. `fleet-create.ts`:
  template = any existing agent's latest version; `restampSlug` rewrites every
  identity string (X-Agent-Name headers, hook args) to the new slug; fresh
  `HERMES_KEY_<SLUG>` appended to the stack `.env`; starter soul. Renderer
  handles `source='created'` defs via a chassis (default `agent-support`
  service block, env `TALARIA_CHASSIS_SERVICE`) with fresh non-external state
  volume + fleet-local dept-skills dir. `retire` action: disable + remove
  container + re-render (bridge drops it live). Verified full loop with a test
  agent (created "remy", answered via gateway, retired; def row remains as
  `retired` with history — re-enable is SQL-only for now).

- **Models tab** - `/models` (System nav): endpoint registry management with
  provider presets (`lib/models.ts` PROVIDER_PRESETS), per-endpoint model
  catalogs (`llm_endpoints.models` jsonb, seeded by the importer), pricing,
  class toggle. Removal is guarded: `fleet-cascade.ts` computes the blast
  radius (`modelUsage`); the API returns 409 `needsForce` for the double
  opt-in, then `cascadeRemoval` strips the tier from each affected agent as a
  NEW version, re-renders, restarts running managed agents. MAIN usage always
  refuses. Agent editor uses `ModelPicker` (one combobox over all catalogs,
  `␟` separator).

- **Full fleet migrated (2026-07-02)** - all 8 agents run Talaria-managed
  (`talaria-fleet` project); every legacy `ai-agent-*` container is stopped and
  retired behind the `retired-migrated-to-talaria` profile in the legacy
  compose (depends_on refs commented; mattermost-bridge's agent-only
  depends_on dropped). Renderer passes compose `secrets:` through, including
  long-form `{source, mode}` entries (dex/dewey `gh_token` — caught live as a
  bogus /run/secrets directory, fixed by re-render + force-recreate).

- **Tier routing** - the manifest lists `<base>-<alias>` entries (Hermes
  `api_server` resolves them); `server/fleet-agents.ts` filters gateway models
  to definition-backed agents + exposes tiers, `routedModelFor` validates;
  `/api/chat` takes `tier`, the composer has a tier select, and
  `usage_events` attributes tier turns by the ALIAS endpoint (verified:
  glm turn → cloud/glm).

- **Pricing** - `llm_endpoints.model_prices` jsonb ({model: {in, out}} $/MTok,
  endpoint price_in/out fallback); `usage_events.endpoint` stamps the serving
  endpoint (backfilled); cost is computed AT READ TIME in `usage.ts` (PRICED
  CTE: local = $0, unpriced cloud = NULL -> surfaced as unpricedCloudTokens).
  Models page has the per-model price grid; Anthropic preset seeds official
  rates. GLM/DeepSeek rates left unpriced deliberately - set them on /models.

## Next up (in order)

1. Remaining stub pages under `_app/`: activity, alerts, skills, memory, mcp,
   inference. Channel-side tier mentions (e.g. `@sam:opus`) later.
2. Decommission cleanup in the legacy stack when ready (commit the retired
   compose there; repoint or retire `sync-agent-state.sh`).
2. **Token-spend + per-LLM-API attribution per ticket** (graph which APIs completed a
   ticket), tracked follow-up to the auto-accumulated time-spent field.
3. Plan chat (turn a channel conversation into tickets on a board).

## Dev environment

- **App:** `cd ui && npm run dev` → http://localhost:5273 (port 5273).
- **Postgres** (durable): container `talaria-postgres-dev` on `:5544`
  (`DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria`).
- **Redis** (sessions + realtime): container `talaria-redis-dev` on `:6399`.
- Both containers are `--restart unless-stopped`, so they survive host reboots. If
  ever stopped: `docker start talaria-postgres-dev talaria-redis-dev`.
- **Migrations** run on boot (idempotent). ⚠️ Gotcha: the migration promise is cached
  per process. If the app boots while Postgres/Redis are **down**, it caches the
  failure and every request 500s until you **restart the dev server** after the DBs
  are up. Always bring up the containers first.
- **Gateway plane:** `TALARIA_GATEWAY_URL=http://127.0.0.1:8642` (bridge container).

## Auth

- Redis-backed sessions (opaque sid cookie → `sess:<sid>`).
- Providers env-gated in `ui/.env`: Google OAuth + username/password.
- **Default admin login:** `jon@packledger.co` / `talaria-dev`
  (`AUTH_USERS` entry whose email is in `AUTH_ADMIN_EMAILS=jon@packledger.co` → admin).
- Agent auth: `TALARIA_AGENT_KEY` (x-api-key / Bearer) for register/heartbeat/report.

## Networking

To expose the dev port on the LAN while developing (firewalld, persistent):

```bash
sudo firewall-cmd --permanent --add-port=5273/tcp
sudo firewall-cmd --reload
# undo later: sudo firewall-cmd --permanent --remove-port=5273/tcp && sudo firewall-cmd --reload
```

## Repo note

The repo now lives at **`outcrop-labs/talaria`** (`https://github.com/outcrop-labs/talaria`),
migrated from `PackLedger/talaria` on 2026-07-02 (the old repo is archived/read-only). `origin`
uses SSH (`git@github.com:outcrop-labs/talaria.git`). This handoff and all docs travel with the repo.

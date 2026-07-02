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

## Next up (in order)

1. Notifications + user @mentions (channels parse agent mentions only today),
   cost/token ledger, admin console (the 6-line stub pages under `_app/`).
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

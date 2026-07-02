# Talaria — handoff for the next agent

_Last updated: 2026-07-02. Scope: Phase 2 (Talaria's own UI). This file is a fast
on-ramp; the authoritative docs are [`docs/PHASE2-UI-PLAN.md`](./docs/PHASE2-UI-PLAN.md),
[`ROADMAP.md`](./ROADMAP.md), [`CHANGELOG.md`](./CHANGELOG.md), and [`ui/README.md`](./ui/README.md)._

## What Talaria is

A self-hosted AI-fleet management product. **Phase 1** = a two-plane framework
(gateway multiplexer + management bridge) in front of hermes-workspace +
mission-control. **Phase 2** (current work) = Talaria's **own** UI in [`ui/`](./ui)
(Vite + TanStack Start, React 19 + TS, Tailwind v4 "Mercury" design system), backed
by Talaria's **own** Postgres/Redis — we *ripped mission-control's functionality into
our stack*, we do **not** proxy MC.

## Current state (what's built in Phase 2)

Full project-management suite, all live in `ui/`:

- **Boards & teams** — shareable kanban boards (personal/team), Board settings modal
  (General / People / Agents), restrictive board-scoped agent policy, archive + delete.
- **Tickets** — TipTap WYSIWYG description (markdown under the hood) with read/edit +
  slide-in full editor, comments (Ctrl+Enter), Activity tab, watchers, quality-review
  gate. Directly-linkable routes `/boards/:boardId/:taskId` + copy-link everywhere.
- **Fields** — priority, effort (XS–XL), multiple assignees (board-scoped agents only),
  dependencies (blocked-by/blocks), labels, due date, auto-accumulated time-spent.
  (Manual hour estimates were removed.)
- **Statuses** — Inbox · Assigned · In progress · **Blocked** · Quality review · Done
  (+ Failed / Cancelled). Drag-and-drop.
- **List view** — configurable, drag-reorderable, click-to-sort columns (persisted per
  board in `localStorage`).
- **Multiplayer** — Redis pub/sub → SSE (`/api/boards/:id/events`).
- **Agent guardrails** — agents can create/triage but **cannot** self-assign
  (`assigned` → 403) or self-complete (`done` → `quality_review`), and can't change
  assignees.

## Next up (in order)

1. **Agent MCP (`talaria-mcp`)** — the immediate next task. Two stages:
   - Agent-authed HTTP: add an agent-key path to `POST /api/boards/:id/tasks`
     (create → `inbox`, board-policy enforced); triage via the already-guarded `PUT`.
   - MCP server exposing only safe tools (`list_boards`, `list_tickets`, `get_ticket`,
     `create_ticket`, `triage_ticket`, `comment`, `report_outcome`, `add_time`,
     `add_dependency`). **No** assign/complete tools — guardrails hold by construction.
2. Chat (agent picker + streaming over the gateway plane), agent/human group chats
   (mini-Slack), notifications + @mentions, cost/token ledger, admin console.
3. **Token-spend + per-LLM-API attribution per ticket** (graph which APIs completed a
   ticket) — tracked follow-up to the auto-accumulated time-spent field.

## Dev environment

- **App:** `cd ui && npm run dev` → http://localhost:5273 (port 5273).
- **Postgres** (durable): container `talaria-postgres-dev` on `:5544`
  (`DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria`).
- **Redis** (sessions + realtime): container `talaria-redis-dev` on `:6399`.
- Both containers are `--restart unless-stopped`, so they survive host reboots. If
  ever stopped: `docker start talaria-postgres-dev talaria-redis-dev`.
- **Migrations** run on boot (idempotent). ⚠️ Gotcha: the migration promise is cached
  per process — if the app boots while Postgres/Redis are **down**, it caches the
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
migrated from `PackLedger/talaria` on 2026-07-02. `origin` points there over HTTPS (SSH was
hanging in this environment; `gh auth setup-git` supplies the credentials). This handoff + all
docs travel with the repo.

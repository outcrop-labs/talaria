# Mission-control lift-source map + Talaria roadmap

> **Lift-source reference.** This documents an upstream we rip features FROM, not a
> backend we run. Mission-control's capabilities (task queue, cost, activity, and
> the rest) were rebuilt in Talaria's own Postgres and Redis. We do not proxy or run
> a live mission-control; this map is the dissection we used to plan what to lift.

Goal: rebuild mission-control's capabilities in Talaria's own stack (Postgres/Redis,
no MC runtime dependency), plus our own additions (**multiple boards, teams, and
board-scoped agents**). MC is a lift source, not a backend.

Source: full dissection of `vendor/mission-control` (2026-07-01), roughly 151 API
endpoints, 50+ tables, 48+ UI surfaces. Legend: ✅ done · 🟡 partial · ⬜ todo ·
⚪ out of scope / deferred (MC/openclaw-specific).

## Agents / fleet
- ✅ Registry + register + heartbeat (Talaria-owned; `fleet_agents`).
- ✅ Assigned-work delivery via heartbeat (`work_items`).
- ✅ Status (offline/idle/busy/error) from heartbeat recency; Fleet + Agents views.
- 🟡 Agent detail (roster shows usage/status). ⬜ tabs: soul, memory, config, tools,
  channels, cron, models, diagnostics, attribution, per-agent keys.
- ⬜ Agent-to-agent messaging (`/api/agents/comms`).
- ⚪ evals / optimize / trust scores / spawn history.

## Tasks / boards (the PM suite)
- ✅ Boards (multiple), membership + **sharing by email** (owner/editor/viewer).
- ✅ **Board-scoped agents** (our addition, only certain agents assignable).
- ✅ Tickets: refs (BOARD-N), status, priority, assignee, due, tags, description,
  estimate/actual hours, outcome/resolution/error, comments (threaded, markdown),
  activity log, watchers.
- ✅ **Quality-review approval gate** (agent → quality_review → human approves → done).
- ✅ Two-pane ticket UI (content + properties rail); kanban with per-column add.
- 🟡 Task list view (kanban only). ⬜ filters (status/assignee/priority/tag/search),
  grouping (by status/priority/agent), **drag-and-drop**, bulk actions, inline edit.
- ⬜ Task queue auto-assignment (`/api/tasks/queue`), outcomes stats, regression view.
- ⬜ @mentions in comments → notifications.
- ⬜ Attachments: image/video upload, reviewable by agents (requested).
- ⚪ GitHub issue/PR sync, branch-into-subtasks, broadcast-runs-OpenClaw.

## Projects → our **boards** already fill this role
- ✅ ticket prefix + counter per board (= MC project ticket refs).
- ⬜ **Teams** (our addition): a team owns/shares boards; users belong to teams.
- ⬜ Board archive, color, deadline.

## Cost / tokens
- ⬜ Token ledger (`token_usage`): record via heartbeat token reporting.
- ⬜ Cost views: overview (timeframe), by-agent, by-session, by-task, trends, export.

## Observability
- 🟡 Activity: per-task log done. ⬜ global activity feed + stats view.
- ⬜ Notifications (@mention/assignment/status/due) + bell + subscriptions.
- ⬜ Standup reports (daily agent rollup).
- ⬜ Alerts (rules on agent/task/session fields → notify/webhook).
- ⬜ System monitor (cpu/mem/disk/gpu/network), pairs with local-inference (P2.5).
- ⬜ Gateway health history.
- ⬜ Global admin views: cross-team boards/tasks + per-agent workload.

## Admin / RBAC / multi-tenancy
- ✅ Users + roles (admin/member) + per-agent access; sessions (Redis).
- 🟡 Admin: role gating in nav. ⬜ Admin UI (users, roles, per-agent access, teams).
- ⬜ Audit trail (immutable action log).
- ⬜ Access requests / approval (invite flow).
- ⚪ Super-admin tenants / OS-user provisioning.

## Automation / integrations
- ⬜ Cron jobs (schedule agent tasks); we have the /schedule + /loop harness ideas.
- ⬜ Webhooks (outbound events).
- ⚪ GitHub, Slack/Discord channels, terminal, office view, security scan, MCP audit.

## Content
- 🟡 Markdown: comments render markdown. ⬜ description + everything rich; **media
  upload (image/video) reviewable by agents** (requested).

---

## Build order (autonomous)
1. **Teams** + board ownership by team (our multi-tenant core).
2. **Task UX depth**: filters, grouping, drag-and-drop, list view, bulk, inline edit.
3. **Attachments** (image/video) + full markdown everywhere, reviewable by agents.
4. **Notifications + @mentions + subscriptions**; global **Activity** feed.
5. **Token/cost ledger** (heartbeat reporting) + **Cost** views.
6. **Admin UI** (users/roles/per-agent/teams) + **global admin** cross-team views.
7. **Agent detail** tabs (soul/memory/config/diagnostics) + agent-to-agent messaging.
8. **Standup**, **Alerts**, **Cron**, **Webhooks**.
9. Deferred ⚪: GitHub sync, channels, terminal, office, super-admin, evals, security.

# Talaria UI

Talaria's own front end: the multiplayer workspace where your people and your
agents share the same boards, tickets, chats, channels, knowledge base, and
artifacts. **Vite + Svelte 5** (TypeScript): an SPA on the client with the API
served from the same origin in dev and prod alike. We do not run
hermes-workspace; it's a parts bin we pull from (the design tokens keep its
contract), and Talaria owns the whole surface and its own state.

- **Design system: Mercury.** A hand-rolled Tailwind v4 token system, Talaria's own
  identity: near-black instrument surfaces, cream readout, one warm gold (`#c8b46c`)
  for brand and action, safety orange reserved for failure/destructive only. Matte,
  no glows. IBM Plex Mono is the chrome voice, IBM Plex Sans the reading voice. Two
  modes, `mercury` (dark) and `mercury-light`. Tokens live in
  [`src/styles.css`](./src/styles.css); the `--theme-*` variable names keep
  hermes-workspace's contract so lifts are painless. Spec:
  [`docs/design/mercury-spec.md`](../docs/design/mercury-spec.md).
- **Auth: pluggable + independently toggleable.** Each provider is enabled only
  when its flag is on and its secrets are present. Google OAuth and
  username/password ship first; the registry ([`src/server/auth/config.ts`](./src/server/auth/config.ts))
  makes adding GitHub/Microsoft/etc. a small change.

## Run it

From the **repo root** — the CLI owns the whole local stack (secrets, `ui/.env`,
the dev containers):

```bash
bun talaria setup   # generates ui/.env + admin credentials, brings up postgres + redis
talaria dev         # the app → http://localhost:5273
```

Full detail and the dev loop: [`../DEVELOPERS.md`](../DEVELOPERS.md). For Google:
create OAuth credentials, set the authorized redirect URI to
`<origin>/api/auth/google/callback`, and the client id/secret land in `ui/.env` via
Admin → Organization once the instance is up.

## Auth surface

| Route | Method | Purpose |
|---|---|---|
| `/api/auth/providers` | GET | Which providers are enabled (drives the login screen) |
| `/api/auth/session` | GET | The current user (or `null`) |
| `/api/auth/google` | GET | Start Google OAuth (302 → consent) |
| `/api/auth/google/callback` | GET | OAuth callback → session cookie → `/` |
| `/api/auth/password` | POST | Username/password login |
| `/api/auth/logout` | POST | Clear the session |

Sessions are **Redis-backed** (`src/server/auth/session.ts`): the cookie carries
only an opaque session id, and the user record lives in Redis under `sess:<sid>`
with a TTL. Logout deletes the key. Admins are designated by `AUTH_ADMIN_EMAILS`
(comma-separated); everyone else who signs in is a member.

## Data + infra

Durable state is **Postgres** (`DATABASE_URL`); sessions and realtime pub/sub are
**Redis** (`REDIS_URL`). Migrations are idempotent and run on boot. Local dev runs
them as containers (`talaria-postgres-dev` on `:5544`, `talaria-redis-dev` on
`:6399`) that `bun talaria setup` creates. If the app can't reach either at boot it
caches a failed migration and every request 500s until restarted — if you bring the
stack up by hand, start the containers first.

## Boards & tickets (project-management suite)

Talaria owns a full PM suite (ripped from mission-control into our own Postgres,
not proxied). Highlights:

- **Boards**: shareable kanban boards, personal or team-owned. Restrictive agent
  policy by default (allow-all is an explicit opt-in). Rename / archive / delete
  live in a consolidated **Board settings** modal (General / People / Agents).
- **Tickets**: rich detail overlay with a WYSIWYG (TipTap) description that stores
  markdown under the hood, read/edit toggle + slide-in full-screen editor,
  comments (Ctrl+Enter to send), an activity tab, watchers, and a quality-review
  approval gate. Each ticket is a **directly-linkable route**
  (`/boards/:boardId/:taskId`) with copy-link affordances on cards, list rows, and
  the overlay.
- **Fields**: priority, agent-appropriate **effort** (XS to XL, not hour estimates),
  **multiple assignees** (board-scoped agents only), labels, due date, **ticket
  dependencies** (blocked-by / blocks), and **auto-accumulated time spent** (agents
  add per-iteration seconds via the API; no manual estimate).
- **Statuses**: Inbox, Assigned, In progress, **Blocked**, Quality review,
  Done (+ Failed / Cancelled). Drag-and-drop across columns; a `blocked` column
  parks stalled/needs-input work.
- **Views**: kanban board, list view, and Gantt; saved views and filters persist
  per board.
- **Multiplayer**: boards are live via Redis pub/sub to SSE (`/api/boards/:id/events`).
- **Teams**: create teams and manage members; team boards are visible to all members.

### Agent guardrails (human sign-off)

Agents authenticate with their own per-agent credentials (`tak_` keys; the legacy
org-wide `TALARIA_AGENT_KEY` is a migration window — see
[`AGENT-KEY-MIGRATION.md`](../docs/AGENT-KEY-MIGRATION.md)). On `PUT
/api/tasks/:id` they may triage (priority, effort, labels, description, status →
`in_progress`/`blocked`/`quality_review`) but **cannot** move a ticket to
`assigned` (403) or `done` (coerced to `quality_review`), and **cannot** change
assignees. Assignment and sign-off stay human. (The dedicated agent MCP that
exposes only these safe operations is shipped — see [`../mcp/README.md`](../mcp/README.md).)

For what's shipped and what's next, see the top-level [`README.md`](../README.md)
and [`ROADMAP.md`](../ROADMAP.md) — they are the only status lists; this file
stays out of that business.

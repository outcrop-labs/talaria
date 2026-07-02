# Talaria UI

Talaria's own front end: the multiplayer workspace where your people and your
agents share the same boards, tickets, and (soon) chats and plans. **Vite +
TanStack Start** (React 19 + TypeScript). We picked the same stack hermes-workspace
uses so we can lift its chat/agent components straight into our app with minimal
friction. To be clear, we do not run hermes-workspace; it's a parts bin we pull
from, and Talaria owns the whole surface and its own state.

- **Design system: Mercury.** A hand-rolled Tailwind v4 token system in the same
  cyberpunk-HUD family as hermes-workspace (so lifted chat components drop in), but
  Talaria's own identity: Mercury-the-planet neutrals (graphite / basalt / regolith)
  with a violet-to-magenta neon accent. Two modes, `mercury` (dark) and
  `mercury-light`. Tokens live in [`src/styles.css`](./src/styles.css); the
  `--theme-*` variable names match hermes-workspace's contract on purpose, purely so
  lifts are painless.
- **Auth: pluggable + independently toggleable.** Each provider is enabled only
  when its flag is on and its secrets are present. Google OAuth and
  username/password ship first; the registry ([`src/server/auth/config.ts`](./src/server/auth/config.ts))
  makes adding GitHub/Microsoft/etc. a small change.

## Run it

```bash
cp .env.example .env      # set AUTH_SECRET + enable a provider (Google first)
npm install
npm run dev               # http://localhost:5273
```

For Google: create OAuth credentials, set the authorized redirect URI to
`<origin>/api/auth/google/callback`, and put the client id/secret in `.env`.

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
(comma-separated); everyone else who signs in is a member. The default self-host
admin is the `AUTH_USERS` entry whose email is also in `AUTH_ADMIN_EMAILS`.

## Data + infra

Durable state is **Postgres** (`DATABASE_URL`); sessions and realtime pub/sub are
**Redis** (`REDIS_URL`). Migrations are idempotent and run on boot. For local dev
they run as containers (`talaria-postgres-dev` on `:5544`, `talaria-redis-dev` on
`:6399`, both `--restart unless-stopped`). If the app can't reach either at boot it
caches a failed migration and every request 500s until restarted, so bring the
containers up first (`docker start talaria-postgres-dev talaria-redis-dev`).

## Boards & tickets (project-management suite)

Talaria owns a Plane/Linear-style PM suite (ripped from mission-control into our
own Postgres, not proxied). Highlights:

- **Boards**: shareable kanban boards, personal or team-owned. Restrictive agent
  policy by default (allow-all is an explicit opt-in). Rename / archive / delete
  live in a consolidated **Board settings** modal (General / People / Agents).
- **Tickets**: rich detail modal with a WYSIWYG (TipTap) description that stores
  markdown under the hood, read/edit toggle + slide-in full-screen editor,
  comments (Ctrl+Enter to send), an activity tab, watchers, and a quality-review
  approval gate. Each ticket is a **directly-linkable route**
  (`/boards/:boardId/:taskId`) with copy-link affordances on cards, list rows, and
  the modal.
- **Fields**: priority, agent-appropriate **effort** (XS to XL, not hour estimates),
  **multiple assignees** (board-scoped agents only), labels, due date, **ticket
  dependencies** (blocked-by / blocks), and **auto-accumulated time spent** (agents
  add per-iteration seconds via the API; no manual estimate).
- **Statuses**: Inbox, Assigned, In progress, **Blocked**, Quality review,
  Done (+ Failed / Cancelled). Drag-and-drop across columns; a `blocked` column
  parks stalled/needs-input work.
- **Views**: kanban board + a **list view with configurable, drag-reorderable,
  click-to-sort columns** (persisted per board in `localStorage`).
- **Multiplayer**: boards are live via Redis pub/sub to SSE (`/api/boards/:id/events`).
- **Teams**: create teams and manage members; team boards are visible to all members.

### Agent guardrails (human-in-the-loop)

Agents authenticate with `TALARIA_AGENT_KEY` (x-api-key / Bearer). On `PUT
/api/tasks/:id` they may triage (priority, effort, labels, description, status →
`in_progress`/`blocked`/`quality_review`) but **cannot** move a ticket to
`assigned` (403) or `done` (coerced to `quality_review`), and **cannot** change
assignees. Assignment and sign-off stay human. (A dedicated agent MCP that exposes
only these safe operations is the next milestone, see [`../ROADMAP.md`](../ROADMAP.md).)

## Where this is headed

The PM suite ships today. From here we grow this app into the full multiplayer
workspace from the [top-level README](../README.md), with human-in-the-loop
guardrails throughout:

- **Agent MCP** 🚧 (`talaria-mcp`): the next milestone. Exposes only the safe
  create/triage tools so agents can work the boards without ever self-assigning or
  self-completing.
- **Group chat** 🔭: multi-agent and human channels, Slack-style, with your agents
  as real members. We lift the chat/agent UX from hermes-workspace into this app.
- **Plan chat** 🔭: humans and agents plan together, then turn the conversation into
  real tickets dropped onto any connected board.
- **On-the-fly artifacts** 🔭: spin up artifacts and pin them to the tickets they
  belong to, straight from chats and work sessions.
- **Agent internals in-app** 🔭: manage each agent's soul, memory, skills, tools, and
  MCP servers right here, version controlled and easy to roll back.
- **Analytics + ROI** 🔭: token cost and ROI tied to the work agents actually shipped,
  across boards, chats, and projects.

## TODO / backlog

- **More auth providers:** GitHub, Microsoft/Entra, generic OIDC. Each drops into
  the provider registry.
- **Hash password credentials** (bcrypt/argon2) instead of plaintext `AUTH_USERS`.
- **Local-inference monitoring:** surface self-hosted inference stacks
  (Ollama, vLLM, llama.cpp, LM Studio, TGI, and friends): health, loaded models,
  GPU/VRAM, tokens/sec, so Talaria becomes an all-in-one self-hosted Hermes
  super-dashboard.
- **Multitenancy:** run several businesses from one Talaria and swap between them in
  a click.

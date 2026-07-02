# Changelog

All notable changes to Talaria. Milestone labels refer to [`PLAN.md`](./PLAN.md).

## [Unreleased]: Phase 2 UI (2026-07-02)

Talaria's own front end ([`ui/`](./ui), Vite + TanStack Start) grows a full,
self-hosted **project-management suite**, owned in Talaria's own Postgres/Redis,
not proxied from mission-control.

### Added
- **Boards & teams**, shareable kanban boards (personal or team-owned), a
  consolidated **Board settings** modal (General / People / Agents), board-scoped
  agent policy (restrictive by default), teams + member management, and soft
  **archive** for boards and tickets.
- **Tickets**, rich detail modal: TipTap WYSIWYG description (markdown under the
  hood) with read/edit toggle + slide-in full-screen editor, syntax-highlighted
  code + hover links, comments (Ctrl+Enter to send), an **Activity** tab, watchers,
  and a quality-review approval gate.
- **Ticket routes**, each ticket is a directly-linkable nested route
  (`/boards/:boardId/:taskId`) with copy-link buttons on cards, list rows, and the
  modal.
- **Fields**, agent-appropriate **effort** (XS-XL), **multiple assignees**
  (board-scoped agents only), ticket **dependencies** (blocked-by / blocks), labels,
  due date, and **auto-accumulated time-spent** (`addTimeSpentSeconds`). Manual hour
  estimates removed.
- **Blocked status**, a new kanban column for stalled / needs-input work.
- **List view**, configurable, drag-reorderable, click-to-sort columns, persisted
  per board.
- **Multiplayer**, live boards via Redis pub/sub → SSE (`/api/boards/:id/events`).
- **Reusable UI primitives**, `CloseButton`, `CopyLinkButton`, `InlineCreate`,
  `danger` button variant; `RichEditor` gains `bare` / `fill` / `onSubmit` modes.
- **Agent guardrails**, on `PUT /api/tasks/:id`, agents may triage but cannot
  self-assign (`assigned` → 403) or self-complete (`done` → `quality_review`), and
  cannot change assignees.
- **Agent MCP (`talaria-mcp`)** ([`mcp/`](./mcp)), an MCP server exposing only the
  safe tools (`list_boards`, `list_tickets`, `get_ticket`, `create_ticket`,
  `triage_ticket`, `comment`, `report_outcome`, `add_time`, `add_dependency`) — no
  assign, no complete; guardrails hold by construction. Identity via
  `TALARIA_AGENT_KEY` + `TALARIA_AGENT_NAME`.
- **Agent-authed task API**, the fleet key plus a new `x-agent-name` header opens
  an agent path on boards list, board tasks (list + create → `inbox`, never
  assigned), ticket detail, comments, and add-dependency. Every agent call is
  checked against the board's agent policy and attributed to the named agent in
  activity/comments. Named agents on `PUT /api/tasks/:id` are policy-checked too;
  unnamed key callers (legacy plugin heartbeat/report) keep their old access.
- **Group chat (channels)**, Slack-style channels where teammates and fleet
  agents are members. Channels + members + agents + messages live in Talaria's
  Postgres; live over Redis pub/sub → SSE (`/api/channels/:id/events`). Agents
  reply when **@mentioned** (by name or model id): the reply streams into the
  channel for every member, built from the channel transcript via the gateway
  plane. Composer has @mention autocomplete; channel settings manage people +
  agents (adding an agent requires access to it). New "Channels" nav surface.

- **People pickers**, one searchable `UserPicker` (over `GET /api/users`, everyone
  who has signed in) replaces every type-an-email field: board sharing, board
  creation invites, teams, and channel members.
- **Label picker**, ticket labels are tag chips + the shared combobox: the board's
  existing labels surface for reuse, and typing creates a new one (Enter or comma).
  Replaces the raw comma-separated text input.
- **Display names**, users can set how they appear (Settings → profile; updates the
  live session, no re-login). Member lists, channel messages, and avatars prefer
  the name and show the email as secondary.
- **Consistent control sizing**, one `sm`/`md` scale (`h-9`/`h-11`) shared by
  Button, Input, Select, and Combobox via a `size` prop — mixed-height form rows
  and hand-set `h-8`/`h-9` overrides are gone.

### Fixed
- SSE event streams no longer crash the server when a client disconnects before
  the Redis subscriber finishes connecting (unhandled rejection in the
  board/channel event stream).
- Ticket labels no longer require hand-typed comma lists (see label picker).

### Changed
- Sessions are Redis-backed (opaque sid → `sess:<sid>`), not HMAC cookies.

## [Unreleased]: 0.1.0

Initial working slice: hermes-workspace ↔ mission-control bridge + per-agent adapter plugin.
All milestones below verified live against a running stack on 2026-07-01
([`scripts/verify-stack.sh`](./scripts/verify-stack.sh), all checks pass).

### Added
- **Gateway plane** (`bridge/src/gatewayPlane.ts`), the fleet multiplexer. Fronts the Hermes gateway
  `:8642` for a whole fleet: `/v1/models` = every agent, `/v1/chat/completions` routed by model to that
  agent's real gateway (per-agent key, SSE streamed). One workspace talks to every agent via the model
  switcher. Fleet declared in a manifest (`TALARIA_FLEET`/`TALARIA_FLEET_FILE`). Stood up the Phase-1
  fleet engine as a two-plane runtime (gateway multiplexer + dashboard management bridge).
- **Bridge** (`bridge/`, Node/TS), transparent reverse-proxy of the Hermes dashboard `:9119`.
  - **M1** pass-through: all 164 dashboard routes (incl. OAuth + 4 websockets) proxied byte-for-byte
    to the real dashboard; conductor capability-probe (`GET /api/conductor/missions`) served as
    `200 application/json` so the workspace uses remote dispatch instead of native-swarm.
  - **M2** mission create: `POST /api/conductor/missions {name,prompt}` → mission-control
    `POST /api/tasks`, response shaped for the Conductor.
  - **M3** status round-trip: `GET/DELETE /api/conductor/missions/{id}` poll + cancel, mapping
    mission-control task status → the workspace mission enum. Never forces the Aegis-gated `done`.
- **Fleet board**, the bridge serves the workspace's `/api/plugins/kanban/*` surface from
  mission-control, so the swarm/kanban board becomes a live view of the MC fleet (columns = MC
  statuses, cards = MC tasks, full CRUD; `done` stays Aegis-gated). Toggle `TALARIA_KANBAN_FROM_MC=0`.
  The conductor poll's `lines` are also enriched with the task's header + comment feed.
- **Plugin** (`plugin/talaria/`, Hermes standalone), per-agent mission-control adapter.
  - **M3** register (`POST /api/agents/register`) + opt-in background heartbeat
    (`TALARIA_HEARTBEAT_SECONDS`) that polls `/api/agents/{id}/heartbeat` + reports via
    `PUT /api/tasks/{id}` (toward `quality_review`, never `done`). Safe no-op until configured.
- **mission-control adapter** (`adapter/`), **M4** `HermesAdapter` making Hermes a first-class
  framework in mission-control; PR-ready patch + verification.
- **Stack** (`stack/`), compose wiring workspace + mission-control + bridge on the shared `edge`
  network; `scripts/verify-stack.sh` reproduces the M1-M3 verification end-to-end.
- **Docs**, [`docs/m0-contract.md`](./docs/m0-contract.md) (the M0 contract diff + `:9119` allowlist),
  README with architecture + compatibility matrix.
- **Fleet: both Hermes deployment shapes** (`bridge/src/config.ts`, `gatewayPlane.ts`, `sessions.ts`) -
  a fleet entry now supports (A) separate installs (one gateway per agent) and (B) multiple Hermes
  profiles on one host (each profile's API server on its own port). Optional `profile` / `upstreamModel`
  / `pathPrefix` fields: Talaria rewrites the forwarded `model` to the profile when set and honours a
  profile path prefix on chat + session calls. The UI only ever sees Talaria's exposed model ids.
  `pathPrefix` seeds support for Hermes' emerging single-endpoint profile multiplex (`multiplex_profiles`).
- **Phase 2 UI** (`ui/`), the first slice of Talaria's own front end (Vite + TanStack Start, React 19,
  TypeScript), matching the hermes-workspace stack so its chat components lift cleanly.
  - **Mercury design system** (`ui/src/styles.css`, `ui/src/lib/theme.ts`), hand-rolled Tailwind v4
    tokens, dark (`mercury`) + light (`mercury-light`), violet→magenta neon on Mercury-planet neutrals.
    Reuses hermes-workspace's `--theme-*` token contract to keep component lifts frictionless.
  - **Pluggable auth**, each provider independently enable-able (flag **+** secrets required):
    registry (`ui/src/server/auth/config.ts`), stateless HMAC-signed sessions, **Google OAuth**
    (start + callback) and username/password, with routes `/api/auth/{providers,session,google,
    google/callback,password,logout}`. Login screen renders only the enabled providers. Verified live:
    session round-trip, logout, tampered-cookie rejection, provider gating.
  - Both upstreams vendored under `vendor/` (gitignored) as lift sources.

### Key findings (verified against source)
- The Hermes dashboard has **no** `/api/conductor/*` routes, Talaria *serves* them (adds capability),
  it does not override native behavior. Unset `HERMES_DASHBOARD_URL` → 100% native.
- mission-control gates `done` behind **Aegis** approval; Talaria respects it (human-only Done).
- mission-control has no published image (builds from source, pinned `d09e608`).

### Not yet
- Decomposed/broadcast mission parity (those go workspace-local `:3000`), needs the
  `HERMES_MISSION_API_URL` upstream PR to hermes-workspace.
- Executing pulled work inside the Hermes run loop (heartbeat pulls; in-agent dispatch is next).
- Enabling the plugin on the live PackLedger fleet (staged, not yet `--force-recreate`d).

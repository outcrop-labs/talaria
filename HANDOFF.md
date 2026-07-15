# Talaria - handoff for the next agent

_Last updated: 2026-07-09. Scope: Talaria as the fleet's harness + product UI. This file is a fast
on-ramp; the authoritative docs are [`docs/TODO.md`](./docs/TODO.md),
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

Underneath the app is the fleet, the Hermes agent containers Talaria renders and
manages. Everything routes through Talaria's own **gateway**: agents call it for
LLM completions (Talaria routes to the providers you register on `/models`), and
Talaria reaches each agent's persona gateway directly for chat. The older bridge
multiplexer / mission-control / conductor bits are **legacy Phase-1 scaffolding**,
now removed from the architecture.

Heads up: Talaria is a work in progress, not production ready. Shipping today: the PM
suite, the unified **Comms** surface (channels · relays · DMs, with unread read-cursors,
DM notifications, and distill-then-archive decay), the **multiplayer Plan view**
(shared plan conversations + living plan document with member grants, presence, and
templated, dependency-aware ticket drafting), **org identity**, **personal assistants**
(identity proxy for owner governance, privacy-gated group channels, admin elevation),
the fleet engine and full agent harness
(federate/design/render/orchestrate/create/retire, versioned config + MCP edits,
skills/memory management, **zero-downtime rolling replacement**, brain-routability
alerts), the token ledger with auto-fetched pricing, activity/alerts, and auth.

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
- **Agent MCP (`talaria-mcp`)** - MCP server in [`mcp/`](./mcp) (TS, built with
  `npm run build`; stdio for one agent, or fleet **streamable-HTTP** mode via
  `MCP_HTTP_PORT` — per-request identity by `x-agent-name`, self-hosted by the app
  via `server/mcp-service.ts` and injected into every rendered config). Exposes only
  the safe tools — tickets, documents/artifacts, channels, KB, search, Google
  confirm-sends, and board governance for personal assistants (see
  [`mcp/src/index.ts`](./mcp/src/index.ts) for the full set); no assign/complete
  tools. Backed by agent-key HTTP paths: `TALARIA_AGENT_KEY` + an `x-agent-name`
  header, board agent policy enforced everywhere (elevated assistants pass), activity
  attributed to the named agent. Create lands in `inbox`, always unassigned. Unnamed
  key callers (legacy plugin heartbeat/report) keep their old access on
  `PUT /api/tasks/:id`.
  See [`mcp/README.md`](./mcp/README.md) for client config.
- **Chat (1:1)** - the home surface: agent picker over the fleet manifest's
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

- **Notifications** - user @mentions in channels, plain **DM messages** (kind `dm`,
  deduped while one sits unread, deep-linking via `/comms?c=<id>`), and **plan shares**
  (kind `plan-share`, `/plan?p=<id>`) land in the **Inbox** (`/inbox`, unread badge in
  the nav; 30s poll). `server/notifications.ts` + `GET/PUT /api/notifications`; the
  composer autocompletes members and agents. Mention tokens: email localpart / dashed
  name / first name (`userMentionTokens` in `server/mentions.ts` — the composer mirrors
  it). Channel unread badges ride `channel_members.last_read_seq` read cursors
  (`POST /api/channels/:id/read`).
- **Token ledger** - `usage_events` (one row per agent generation), recorded from
  both persist paths (`chat-persist.ts`, `channel-replies.ts`). Real counts via
  `stream_options.include_usage` (the agent gateways honour it — prompt tokens run
  ~17-35k/turn because each turn carries the agent's full context); char/4
  estimates flagged when absent. `GET /api/cost` + the `/cost` page (tiles, 14-day
  strip, per-agent). Dollar cost needs per-LLM pricing attribution (next-up #2).

- **Admin console** - `/admin`: user roles (pinned admins via `AUTH_ADMIN_EMAILS`,
  no self-demotion) + per-person agent allow-lists (`user_agent_access`; empty =
  all agents). `GET/PUT /api/admin/users`, admin-gated.

- **Agent harness (phase A)** - Talaria as the fleet's source of truth. Tables
  `llm_endpoints` (class local|cloud, feeds the cost split), `agent_defs`,
  `agent_versions` (immutable payloads: soul + config jsonb incl. `raw` full
  config.yaml). Federation `server/fleet-federate.ts` (2026-07-08; replaced
  the old importer) ingests a Hermes-format dir on demand and creates each
  agent NATIVELY: Talaria chassis, fresh key + state volume, skills copied
  into the fleet dir. Admin API `GET /api/fleet/defs`, `POST /api/fleet/federate`,
  `GET /api/fleet/defs/:id/versions`; Definitions panel on `/agents`.
  Key context: Hermes `model_aliases` = one container serving all model tiers
  (that + `fallback_providers` replace per-tier scaffolding); the external
  stack's pain is 6 hand-edited places per agent — the renderer (phase B)
  generates all of them from the version payload.

- **Agent harness (phase B)** - the renderer + orchestrator. `server/fleet-render.ts`
  materializes managed versions into gitignored `fleet/` (config.yaml as **YAML
  1.1** — PyYAML semantics). Every agent renders from ONE Talaria-owned chassis
  (`fleet/chassis.yml`: service block + per-slug extras + a `network:` name — now
  the single unified `talaria` network for every Talaria container). Every model
  spec in the rendered config.yaml is rewritten to route through Talaria's own
  gateway (`base_url ${LLM_BASE_URL}`), so agents have exactly one upstream. Each
  agent's persona gateway is published on a stable loopback port
  (`agent_defs.gateway_port`), and the renderer writes `fleet/fleet.json` (model →
  each agent's url + key) which Talaria reads to reach agents **directly** — no
  bridge/multiplexer. Per-agent secrets (`agent_secrets`, secretbox-encrypted,
  Secrets tab) materialize into `fleet/agents/<slug>/secrets.env` (0600) wired via
  env_file. `server/fleet-docker.ts` drives `docker compose -p talaria-fleet` with
  `--env-file fleet/.env`. Lifecycle API `POST /api/fleet/agents/:id/control`
  (up/stop/retire/unretire; owners of a personal assistant may up/stop their own),
  containers API, and live status + buttons on `/agents`. Imported agents keep
  their pre-Talaria state VOLUME NAMES (`ai_hermes-<dept>`, external) so no memory
  was lost in the migration — the one deliberate docker-level remnant.

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
  `HERMES_KEY_<SLUG>` appended to the fleet `.env`; starter soul (or a
  Muse-designed one — the create flow drafts a whole agent from a description).
  Templates are optional: platform defaults build the config from the first
  local endpoint. All agents render from the chassis with a fresh state volume
  + fleet-local skills dir. `retire` action: disable + remove
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

- **Full fleet on Talaria (migrated 2026-07-02, cord cut 2026-07-08)** - the
  whole fleet runs Talaria-managed (`talaria-fleet` project). The old
  `ai/orchestration` agent stack is fully removed as a code dependency AND from
  its own repo (retired services, agents/, crons/, hooks, skills, bridges all
  deleted); agents' Plane/Outline/Mattermost/gmail-drive MCP connections are
  gone — agent work and chat centralize in Talaria (boards/KB/channels), with
  souls/skills/memories retargeted to match. Chassis extras carry dex/dewey/dot
  workspaces and secrets, including long-form `{source, mode}` entries.

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
  **Auto-fetch**: `server/price-oracle.ts` pulls OpenRouter's public catalog
  (no key) into `llm_endpoints.auto_prices` (background refresh ≤6h on reads;
  immediate on provider add / model change); coalesce chain is user override →
  auto → endpoint default; exact-name match only (vendor-prefix aware,
  dots↔dashes normalized) — no match stays honestly unpriced (e.g. the bare
  `glm` litellm alias).

- **Tier mentions in channels** - `@Dex:deepseek` routes that reply to the
  tier (`mentionedAgents` parses `:tier`, unknown tier falls back to main);
  composer autocompletes `Label:tier`; ledger attributes by the alias endpoint.

- **Activity + Alerts** - `/activity`: merged user-scoped feed (task_activity +
  channel messages + agent_versions) via `server/activity-feed.ts`, kind
  filters, poll. `/alerts`: `server/alerts.ts` computes live (no tables):
  managed container down/unhealthy, gateway unreachable, unpriced cloud tokens,
  estimate-heavy ledger, failed/stale-blocked tickets on the user's boards.

- **Agent internals** - `server/agent-skills.ts` lists/edits skills on the REAL
  mounts, all Talaria-owned (shared `fleet/skills`, per-agent
  `fleet/agents/<slug>/skills`); surfaced as tabs in the agent manage modal
  (the standalone /skills and /memory pages were removed as redundant);
  slug-regex + resolved-prefix traversal guard; edits are live (Hermes reads
  per invocation). `/memory`: `server/agent-memory.ts` reads/writes
  `/opt/data/memories/MEMORY.md` through the running container (docker exec;
  agent writes race human edits — last writer wins, surfaced in UI). `/mcp`:
  `server/agent-mcp.ts` reads `raw.mcp_servers` from the current version;
  add/remove → NEW version via `applyMcpEdits` (X-Agent-Name auto-header,
  untouched entries byte-preserved) + optional render/restart.

- **Inference page** - `/inference`: local endpoints probed live (reuses
  provider-catalog's /models fetch + docker-hostname fallback), latency +
  serving-now chips, local throughput tiles + per-model 30d table.

- **Per-ticket token spend** - `usage_events.task_id`;
  `POST /api/tasks/:id/usage` (agent-key + board policy; humans can't post
  counts) + MCP tool `log_usage` (tier-aware). Reports are normal priced
  ledger rows + a task-activity line; ticket rail shows tokens · $ ·
  per-model via `taskUsage()` in `getTaskFull`.

- **Plan chat** - the **Plan** button in a channel header:
  `POST /api/channels/:id/plan` sends the transcript to a chosen channel agent
  (any tier) with a structured prompt; `server/channel-plan.ts` extracts the
  JSON array (bracket-matching, fence/prose tolerant); the review modal edits
  proposals, picks a board, creates via the normal boards API (inbox, never
  assigned).

- **Legacy stack decommissioned (committed)** - the packledger-services repo
  has the retirement commit: agents behind `retired-migrated-to-talaria`,
  probe/librechat/openwebui/kanban/monitoring removed, `backup.sh` +
  `sync-agent-state.sh` repointed at `talaria-fleet-agent-*-1` containers
  (sync verified live).

- **Local inference topology (settled 2026-07-02)** - the fleet's main model
  routes through the nginx `inference-router:8000` (LB across the two Spark
  boxes). That router is DELIBERATE, SEPARATE inference-plane infrastructure
  (per Jon) — Talaria treats it as one `local` endpoint and does not manage
  its internals. (A brief direct-to-Spark detour was reverted; agent config
  versions record both moves.) litellm's `glm` = `openrouter/z-ai/glm-5.2`,
  manual-priced $0.95/$3.00 per MTok.

- **Talaria LLM gateway (2026-07-02)** - Talaria serves its OWN
  OpenAI-compatible endpoint over the whole registry:
  `/api/llm/v1/models` + `/api/llm/v1/chat/completions` (streaming +
  non-streaming). Model names: bare (`pl-main`, round-robins endpoints that
  serve it) or endpoint-qualified (`openrouter/z-ai/glm-5.2` — first segment
  is the ENDPOINT name). Talaria resolves provider keys server-side
  (provider-catalog `resolveKey`, stack .env; callers never see them) and
  deep-merges each endpoint's `llm_endpoints.request_defaults` under the
  client body — the OpenRouter US no-train allowlist now lives THERE
  (`server/llm-gateway.ts`), not in litellm. Every call is metered:
  `usage_events` source `gateway`, attributed `api:<key name>`, real usage
  from the stream's final chunk. Auth: per-user keys `tlk_…` (sha256 in
  `llm_api_keys`; shown once at mint) — Settings → API keys; minting is
  admin-always + per-user `users.can_mint_keys` grant (checkbox in /admin).
  Remaining to migrate off litellm: agents' cloud ALIAS tiers still point at
  litellm/direct endpoints; pointing them at the gateway needs (a) a
  host-reachable base URL for containers and (b) dedup so a tier turn isn't
  double-counted (chat row + gateway row). identity-proxy (Open WebUI
  binding) is stopped — dead path since Open WebUI retired.

- **Guardrails (shipped 2026-07-08, superseding the 2026-07-02 direction)** -
  the confab guard is now **Talaria-native at the LLM gateway** (`server/guardrails.ts`):
  a pluggable rule registry (zero-tool claims, ungrounded refs, fabricated outages,
  secret leaks) with confidence scoring and off/**observe**/annotate/strict modes.
  The turn's tool record is derived from the request messages, so no agent-side
  trace export is needed; findings land in `guard_findings` (Admin → Confab guard)
  and `guardText()` feeds secret-leak hits into the QA judge on ticket outcomes.
  INVARIANT: the guard is fire-and-forget — never awaited on a completion path,
  and findings are NEVER fed back into a model's context. Coverage is complete:
  `/api/chat` + channel replies run `guardChatReply` (tool-name-only rules, no
  false positives from missing results), and agents' inner tool loops hit the
  full guard at the LLM gateway since the Phase-7 rewire; the agent-side
  confab-guard plugin is gone. Annotate/strict are real (2026-07-15): findings
  pin to the message row (`messages.guard` / `channel_messages.guard`) and
  render as a caveat in chat/channels; the public API route appends the caveat
  (non-streaming) or injects a final SSE delta before `[DONE]` (streaming) —
  but never for agent-loop keys (`gateway_unmetered_keys`); strict also redacts
  detected secrets from whatever is persisted or not yet relayed.

- **Product IA + elegance pass (Phase 5, 2026-07-06)** - nav regrouped into
  **Work** (Home · Chat · Channels · Boards · Inbox) and **Manage** (Agents ·
  Models · Compute · Cost · Audit · Alerts); `/` is now **Home** (triage/review/
  unblock queues + fleet glance, `server/home.ts`), Chat at `/chat`; dead
  `/tasks` removed; `/fleet` → `/agents`; Skills/Memory/MCP reached from the
  Agents-page toolbar. **"Local" → "Self-hosted"** everywhere (DB class value
  still `local`). **Talaria LLM gateway** (`server/llm-gateway.ts`,
  `/api/llm/v1`) + per-user `tlk_` keys (`llm-keys.ts`, Settings; grant via
  `users.can_mint_keys`). **No-train** = per-endpoint toggle writing
  `llm_endpoints.request_defaults`. **Versioned skills/memory**
  (`internal_versions`, `internal-history.ts`, `/api/history`) edited in the
  **WYSIWYG `InternalEditorModal`**. **MCP probe** (`mcp-probe.ts`,
  `/api/mcp/test`) gives live connection status.

- **Product depth (Phase 6, 2026-07-06)** - **unified agent management modal**
  (`components/fleet/agent-manage-modal.tsx`: Summary·Config·Skills·Memory·MCP·
  Versions) reached from the redesigned **grid/list Agents roster** (icon
  controls; health from real container state; editable `agent_defs.role`;
  duplicate + typed-slug retire + `unretire`). **Fleet Reconcile**
  (`fleet-reconcile.ts`, `/api/fleet/reconcile`). **Personal assistants**
  (`agent_defs.owner_user_id`, `personal-agent.ts`, `/api/me/assistant`, Home
  card). **Attachments** (`uploads` table + `messages/channel_messages.attachments`,
  `server/uploads.ts`, `/api/uploads`, `components/chat/attachments.tsx`;
  images → data-URL vision parts). **Per-view access** (`users.denied_views`,
  `GATEABLE_VIEWS`, session exposes it, `_app` route-gates). **Audit trail**
  (`audit_log` + `app_settings`, `server/audit.ts`, `logAudit()` wired into
  governance mutations; Audit page `audit` source for admins; retention setting
  in Admin). **Models page** compacted (provider card → Manage modal w/
  catalog-search add-model). **Modal** now portals to `<body>` (viewport-centered).
  ⚠️ Dev-server restart flakiness this session: start it via a
  `run_in_background` Bash task from the `ui/` cwd and poll
  `until curl .../api/auth/providers; do sleep 4; done` — nohup from the wrong
  cwd silently no-ops.

## Current state (Phase 7, 2026-07-09) — comms, identity, rolling fleet

- **Comms** — Chat + Channels unified into `/comms` (`routes/_app/comms.tsx`; old
  routes redirect). `channels.kind` = `'channel' | 'group' | 'dm'` on ONE
  machinery (members/agents/SSE/mentions/plan). **Relays** (kind `group`) are
  named ad-hoc gatherings that **Conclude** (`/api/channels/:id/conclude` →
  summary posted as the final message + indexed under channel ACL → archive);
  **teammate DMs** (kind `dm`, deduped on sorted user-id pair via `dm_key`,
  `POST /api/dms`); **agent DMs** default to a FRESH thread per topic (bounded
  context; recent threads nest under the agent in the sidebar). Idle agent chats
  **distill then archive** (`server/comms-decay.ts`, `TALARIA_CHAT_TTL_DAYS`
  default 14, hourly-throttled opportunistic sweep off the channels listing;
  failed distillation never archives; plans exempt). Activity-brain ACL grew
  `planOwnerId` + `ownerUserId` owner-scoping clauses (`retrieval/index.ts`).
  Header: user-chip flyover (settings/theme/sign-out); membership pickers with
  avatar stacks live in the pane header.
- **Plan view (#55, shipped)** — plan conversations (`conversations.kind='plan'`)
  + a side-by-side **living plan document** (a real `doc` artifact linked via
  `artifact_links target_type='plan'`; find-or-create server-side, template-
  seeded). "Sync from chat" = the plan's own agent rewrites it
  (`server/plan-doc.ts`, `POST /api/plan/:id/doc`; preamble/fence stripping).
  Draft tickets is board-first, doc-aware (the document is the source of
  truth), and **dependency-aware** (`dependsOn` same-batch indices → real
  `task_dependencies` on create). Plan turns + docs index into the activity
  brain. NOT yet multiplayer — that's the top open thread.
- **Templates** — org library `templates` (kind `ticket|plan`; markdown skeleton
  = the schema + prompt-only guidance), `board_templates` bindings w/ default,
  `agent_defs.ticket_template_id`/`plan_template_id` overrides.
  `resolveTemplate()` chain (explicit → agent → board default → none) applies in
  plan/channel drafting, plan-doc create+sync, and bare ticket creation
  (`server/templates.ts`; library modal + Board settings section + agent
  Summary-tab bindings).
- **Org identity** — Admin → Organization (`org_name`/`org_about` in
  app_settings, `server/org.ts`). Injected into muse generation for identity
  kinds AND prepended to every RENDERED SOUL.md (a projection; stored souls stay
  clean). Org saves propagate via `rollRunningAgents()`.
- **Rolling replacement** — two compose slots per agent (`agent-<dept>` /
  `agent-<dept>-b`, `agent_defs.active_slot`). `rollAgent()`
  (`server/fleet-reconcile.ts`): overlay render (both slots) → incoming
  container on a freshly allocated port → health gate (failure discards the
  newcomer) → DB cutover + re-render (manifest read per call → instant traffic
  shift) → drain (`TALARIA_ROLL_DRAIN_SECONDS`, default 45) → retire old by
  container name. Config-edit + MCP applies and org saves all roll.
  `fleet-docker.ts` resolves the active slot internally so existing call sites
  are untouched; `containerStatus` recognizes either slot. `proxyChat`
  (`server/gateway.ts`) holds-and-retries connection failures for up to 2min
  (manifest re-read per attempt) — no more mock replies mid-restart.
- **Provider catalogs (hardened)** — always live, never maintained lists:
  full `/models` fetch everywhere (Anthropic pagination, Together bare arrays,
  Gemini `models/` prefix normalization; Perplexity has no catalog API),
  live OpenRouter **US no-train pool** from `GET /providers` injected at request
  time (`provider-catalog.ts` + `llm-gateway.ts`), gateway catalog entries carry
  `qualified` (bare ids may contain `/`), and **config saves auto-register**
  picked models on their endpoint from the live catalog (root cause of a
  frozen-chat bug: unregistered model → gateway 404 → agent aborts silently).
  Beware: provider pools CHURN — a model can lose its US pool mid-day.

## Next up

The living backlog is [`docs/TODO.md`](./docs/TODO.md) — much of the old list here
shipped (artifacts incl. file uploads, @mentions, the per-agent Talaria toolkit,
QA judge with template rubric, Talaria-native confab guard, Google Workspace,
personal assistants with identity proxy + admin elevation, Muse, native crons,
federation, per-agent secrets, blank-machine setup, **multiplayer plan view**,
comms with unread badges + DM notifications, brain-routability health, templates,
org identity, rolling replacement).
Highest-leverage remaining threads:

1. **Retrieval quality follow-ons** — hybrid keyword+dense search; an
   embedding-model migration flow (swapping `TALARIA_EMBED_MODEL` changes
   dimensions → needs a guided reindex). Reranking + curation shipped.
2. **Toolkit onboarding skill** — the toolkit MCP is now ATTACHED to every
   agent (fleet HTTP mode, `server/mcp-service.ts` + render injection); what
   remains is the Hermes-side skill teaching agents to reach for it (#78).
3. **Artifact tail** — S3 behind `storage_ref`; attachments on tickets/chat.
4. **Input sweep (#49)**; **explicit plan-template picker**.

(Guard coverage for the direct chat path shipped in #90 and the agent-side
confab-guard plugin is gone; annotate/strict became real on 2026-07-15 — the
remaining guard thread is feedback-into-agent-memory.)

## Dev environment

- **First time:** `./scripts/setup.sh` (generates secrets + admin login + fleet
  config plane), then `./scripts/dev.sh` (infra + app). Day to day: `./scripts/dev.sh`
  → http://localhost:5273.
- **Postgres** (durable): container `talaria-postgres-dev` on `:5544`
  (`DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria`).
- **Redis** (sessions + realtime): container `talaria-redis-dev` on `:6399`.
- Both containers are `--restart unless-stopped`, so they survive host reboots. If
  ever stopped: `docker start talaria-postgres-dev talaria-redis-dev`.
- **Migrations** run on boot (idempotent). ⚠️ Gotcha: the migration promise is cached
  per process. If the app boots while Postgres/Redis are **down**, it caches the
  failure and every request 500s until you **restart the dev server** after the DBs
  are up. Always bring up the containers first.
- **Agent chat:** the app reads `fleet/fleet.json` and calls each agent's persona
  gateway directly on its published loopback port (no bridge/multiplexer).

## Auth

- Redis-backed sessions (opaque sid cookie → `sess:<sid>`).
- Providers env-gated in `ui/.env`: Google OAuth + username/password.
- **Default admin login:** generated by `scripts/setup.sh` (`admin@talaria.local` +
  a random password, printed once at setup). The `AUTH_USERS` entry whose email is in
  `AUTH_ADMIN_EMAILS` becomes admin.
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

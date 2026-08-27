# M0: Contract diff + intercept allowlist (legacy Phase-1 fleet engine)

> **Heads up: this documents the legacy Phase-1 fleet engine.** This is the M0 spike contract for the
> Phase-1 bridge/dashboard allowlist, the seam Talaria used to front hermes-workspace +
> mission-control. That scaffolding is kept for reference, but it is not the product. Talaria's own app
> lives in [`ui/`](../../ui), backed by its own Postgres/Redis; we lifted mission-control's capabilities
> in rather than proxying a running copy, and the current fleet engine is just the gateway plane
> (model-routed chat on `:8642`). The technical contract below is accurate as written for the Phase-1
> bridge; read it as the legacy engine, not Talaria's current shape. See the
> [top-level README](../../README.md) for where things stand.

> **Verdict: weekend to first working round-trip (M2); about 2 to 3 weeks to full Conductor parity
> (M3).** The intercept surface is 3 routes; translation is straight field-mapping; pass-through is a
> stock reverse proxy (already built). Source-read from the three upstreams on 2026-06-30, see
> [Sources](#sources). This supersedes the "unverified bodies" caveat in [`../PLAN.md`](./PLAN-design-notes.md).

## The key architectural correction

The plan assumed Talaria *intercepts* an existing dashboard mission route. It doesn't. **The Hermes
dashboard `:9119` has no `/api/conductor/*` routes at all.** Its `/api/*` catch-all returns 404 for
anything unmatched (`hermes_cli/web_server.py:11042`). Meanwhile hermes-workspace's Conductor *calls*
`POST /api/conductor/missions` on `HERMES_DASHBOARD_URL` and, when it 404s, flips
`capabilities.conductor = false` and falls back to **native-swarm** (local tmux workers on `:3000`,
`src/routes/api/conductor-spawn.ts:449`).

So the bridge's real job: **SERVE the `/api/conductor/*` routes the dashboard lacks** (translating to
mission-control), and **pass everything else through** to the real `:9119`. This makes the
non-destructiveness guarantee *stronger* than the plan claimed:

- The routes the bridge answers are ones the real dashboard **404s anyway**, so we add capability,
  never override native behavior.
- Unset `HERMES_DASHBOARD_URL`, workspace 404s conductor, native-swarm. 100% native, as before.
- Every other `:9119` route (164 of them) proxies byte-for-byte, including OAuth + websockets.

```
workspace Conductor ──POST /api/conductor/missions──► BRIDGE ──translate──► mission-control POST /api/tasks
                     ──(all other /api/*, /assets, ws)─► BRIDGE ──pass-through──► real :9119 dashboard (kanban-dashboard)
```

## Intercept allowlist (the safety contract)

**SERVE (the bridge answers; do NOT forward):**

| Method | Path | Purpose | Translates to (mission-control) |
|---|---|---|---|
| POST | `/api/conductor/missions` | create a mission | `POST /api/tasks` |
| GET | `/api/conductor/missions/{id}` | poll mission status (`?lines=`) | `GET /api/tasks/{id}` (map status) |
| DELETE | `/api/conductor/missions/{id}` | cancel mission | `PUT /api/tasks/{id}` `{status:"done"|cancelled}` |

**PLUS the capability probe, RESOLVED (M1, verified live 2026-07-01).** The workspace decides
`capabilities.conductor` via `probeConductor` (`gateway-capabilities.ts:581`): it sends
**`GET /api/conductor/missions`** and marks conductor **available iff the response is `200` +
`Content-Type: application/json`** (`404/405` → native-swarm; a `200 text/html` SPA-fallback →
native-swarm; `401` → available). So the bridge serves the bare `GET /api/conductor/missions` with
`200 {"missions":[]}`. Confirmed against a live workspace: on boot it probes, the bridge serves 200
JSON, and the workspace flips to remote dispatch. (When `mc.enabled` is false the bridge returns 503
on the probe, so the workspace correctly falls back to native-swarm.)

**PASS THROUGH (everything else, byte-for-byte):** all 164 dashboard routes, `/api/status`,
`/api/sessions/*`, `/api/config/*`, `/api/skills/*`, `/api/mcp/*`, `/api/cron/*`, `/api/profiles/*`,
`/api/models/*`, `/api/dashboard/*`, `/assets/*`, the SPA fallback `/{path}`, and the 4 websockets
(`/api/pty`, `/api/ws`, `/api/pub`, `/api/events`). Allowlist-intercept: any route not matching
`/api/conductor/*` is forwarded untouched, including routes future Hermes versions add.

## Field-by-field: workspace mission → mission-control task

**Workspace → bridge** (`POST /api/conductor/missions`, `conductor-spawn.ts:116`):
```jsonc
{ "name": "conductor-1735689600000",   // mission label
  "prompt": "<orchestrator system prompt + goal>" }
```
**Bridge → mission-control** (`POST /api/tasks`, `src/app/api/tasks/route.ts:174`):
```jsonc
{ "title":       "<name>",             // required
  "description": "<prompt>",
  "status":      "inbox",              // default; MC workflow: inbox→assigned→in_progress→quality_review→done
  "priority":    "medium",             // default
  "metadata":    { "talaria": { "source": "hermes-workspace", "mission_name": "<name>" } } }
```
**mission-control → bridge → workspace:** MC returns `{ task: { id, ticket_ref, status, ... } }`;
the bridge shapes the conductor response the workspace reads (`conductor-spawn.ts:122`):
```jsonc
{ "id": "<task.id>", "name": "<name>", "session_id": "<task.ticket_ref or id>" }
```
**Status round-trip** (`GET /api/conductor/missions/{id}`): map MC task `status` →
workspace mission state. Suggested map: `inbox|assigned`→`planning`, `in_progress`→`executing`,
`quality_review`→`reviewing`, `done`→`complete`. (TODO(M3): confirm the exact state enum the
workspace UI expects for a *remote* mission vs the local `SwarmMission.state`.)

> **Scope finding (drives M3 + the upstream PR):** the workspace's *decomposed* and *broadcast*
> flavors go to its **local** `:3000` `/api/swarm-decompose` + `/api/swarm-dispatch`, **not** the
> dashboard, so a dashboard-proxy only captures the single Conductor "mission" path. Full parity
> (decomposition/broadcast → mission-control) needs either the upstream `HERMES_MISSION_API_URL` PR
> (recommended) or a second seam on the local endpoints. **M2 beachhead = single mission; M3 = parity.**

## Per-agent plugin ↔ mission-control (the adapter half)

Confirmed endpoints (correcting the scaffold's stubs, which used wrong paths):

| Op | mission-control endpoint | Body / notes |
|---|---|---|
| register | **`POST /api/agents/register`** | `{ name, role:"agent", capabilities:[], framework:"hermes" }` → `{ agent:{id,...}, registered }` |
| poll | `GET /api/agents/{id}/heartbeat` | → `{ status:"HEARTBEAT_OK"\|"WORK_ITEMS_FOUND", work_items:[{type,count,items:[…]}] }` |
| report | **`PUT /api/tasks/{id}`** | `{ status, outcome?, error_message?, resolution?, actual_hours? }` (NOT a `/report` subpath) |
| message | `POST /api/agents/message` | `{ from, to, content, type? }` (inter-agent, M3) |

**Auth (both bridge + plugin):** `x-api-key: <key>` header (also accepts `Authorization: Bearer`).
Global `API_KEY` = admin; per-agent keys live in MC's `agent_api_keys` (scoped roles). Task
create/report needs `operator+`. `src/lib/auth.ts:580`.

## mission-control Hermes adapter (M4 target)

MC already defines `FrameworkAdapter` (`src/lib/adapters/adapter.ts:56`): `register` / `heartbeat` /
`reportTask` / `getAssignments` / `disconnect`, each broadcasting via `eventBus`. A `hermes` adapter
mirrors `claude-sdk.ts`. MC even ships a nascent `/api/hermes` hook integration
(`.hermes/hooks/mission-control/`). Clean upstream PR, see `../adapter/README.md` (deleted with the adapter; this link kept as text).

## Deploy facts (corrects the stack, M5)

| | image | port | key env | data |
|---|---|---|---|---|
| **hermes-workspace** | `ghcr.io/outsourc-e/hermes-workspace:latest` ✅ published | `3000` | `HERMES_API_URL`, `HERMES_DASHBOARD_URL`, `HERMES_API_TOKEN`, `HERMES_PASSWORD` (required if non-loopback), `HERMES_HOME`, `HERMES_WORKSPACE_DIR` | vols `.hermes`, `/workspace` |
| **mission-control** | ⚠️ **no published image**, builds locally (`mission-control:latest`) | `3000` | `PORT`, `API_KEY` (auto-gen), `AUTH_SECRET` (auto-gen), `OPENCLAW_GATEWAY_HOST/PORT` | SQLite `/app/.data/mission-control.db`, vol `mc-data:/app/.data`; read-only rootfs + cap_drop hardening |

Stack compose (in the `stack/` directory, since removed from the repo) was updated to match:
workspace uses the real ghcr image;
mission-control switched to a **build context** (must `git clone builderz-labs/mission-control` +
`docker build`, vendoring TBD) with its real env + data volume.

## Sources

Source-read 2026-06-30 (shallow clones in scratch; not vendored):
- **hermes-workspace** `outsourc-e/hermes-workspace` v2.3.0, `src/server/gateway-capabilities.ts`,
  `src/routes/api/conductor-spawn.ts`, `swarm-dispatch.ts`, `swarm-decompose.ts`, `docker-compose.yml`.
- **mission-control** `builderz-labs/mission-control`, `openapi.json`, `src/app/api/{tasks,agents}/*`,
  `src/lib/{auth,adapters}/*`, `docker-compose.yml`, `Dockerfile`.
- **hermes-agent** `NousResearch/hermes-agent` (vendored `ai/hermes-upstream`),
  `hermes_cli/web_server.py` (164 routes + 4 ws), `dashboard_auth/{routes,middleware,public_paths}.py`.

## Status

- **M0 ✅** contract + allowlist (this doc).
- **M1 ✅ (verified live 2026-07-01):** pass-through proxy works against the live `kanban-dashboard`;
  the conductor capability-probe is `GET /api/conductor/missions` (200+JSON ⇒ available), now served.
- **M2 ✅ (verified live 2026-07-01):** `POST /api/conductor/missions {name,prompt}` through the bridge
  created mission-control task `id:1` `TASK-001` (title=name, description=prompt, `metadata.talaria`
  stamped); bridge returned `{id, name, session_id:"TASK-001"}`. Single mission round-trips to the board.

  *How it was verified:* the `stack/` compose (since removed from the repo) brought up on the `edge` net (bridge → live `kanban-dashboard`
  pass-through; workspace → bridge; mission-control built from `talaria/vendor/mission-control`). Auth:
  bridge + mission-control share `MISSION_CONTROL_API_KEY` (`x-api-key`).
- **M3 poll/cancel ✅ (verified live 2026-07-01):** `GET /api/conductor/missions/{id}` returns the
  mission record the Conductor UI renders (`conductor-spawn.ts:290`), `{id,name,status,error,
  session_id,lines,exit_code,updatedAt}`, with `status` mapped from the mission-control task:
  `done|outcome=success → completed`, `outcome=abandoned → cancelled`, `outcome=failed|error_message →
  failed`, else `running` (workspace enum: `running|completed|failed|cancelled`, terminal sets in
  `use-conductor-gateway.ts`). `DELETE /api/conductor/missions/{id}` cancels via
  `PUT outcome=abandoned`. Verified: create→poll `running`→cancel→poll `cancelled` (exit_code 1);
  unknown id → 404.

  **Governance finding:** mission-control gates the `done` transition behind **Aegis approval**
  ("Aegis approval is required to move task to done"). Talaria deliberately does NOT bypass it, it
  never forces `done`; completion flows through mission-control's (human) approval, which maps to
  `completed` on the next poll. This matches PackLedger's human-only-Done posture ([[plane-lifecycle]]).

- **M3 adapter half ✅ (verified live 2026-07-01):** the Talaria **plugin** register/heartbeat/report
  cycle works against mission-control. Register = `POST /api/agents/register`
  `{name, role, capabilities, framework:"hermes"}` → `{agent:{id}}`; a background heartbeat thread
  (opt-in `TALARIA_HEARTBEAT_SECONDS`) polls `GET /api/agents/{id}/heartbeat` and logs
  `WORK_ITEMS_FOUND`; `post_tool_call` reports via `PUT /api/tasks/{id}` (moves toward
  `quality_review`, never `done`, Aegis-gated). Verified standalone: register → agent appears in MC →
  assigned task pulled by heartbeat (`4 work item(s)`) → report moved the task to `in_progress`.
  Fleet wiring staged (no-op until enabled): `TALARIA_MISSION_CONTROL_URL` +
  `TALARIA_HEARTBEAT_SECONDS` on the `&agent-env` anchor; per-agent `TALARIA_AGENT_ROLE`.

## Open items (ranked)

1. **M3, decomposition/broadcast parity:** those dispatch flavors are workspace-local (`:3000`),
   not via the dashboard, full parity needs the `HERMES_MISSION_API_URL` upstream PR (recommended)
   or a second local seam. (Single-mission conductor path is done.)
2. **M4, execute pulled work:** the heartbeat pulls assigned tasks but does not yet dispatch them
   INTO the Hermes run loop (deeper integration). Today the plugin makes each agent a live, reporting
   fleet node; autonomous execution of pulled work is next.
3. **M3, mission log lines:** the poll record's `lines: []` is empty (mission-control has no conductor
   log stream); optionally surface task comments/activity there.
4. **Enable on the live fleet:** set `TALARIA_MISSION_CONTROL_URL` + `TALARIA_HEARTBEAT_SECONDS` and
   `docker compose up -d --force-recreate` the 8 agents (deferred, not yet done).
5. **M5:** pin the mission-control build (commit) + fold in upstream hardening; compatibility matrix.

# API reference — fleet

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

27 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/fleet`](#apifleet) | GET | `session` + `view:/observability` |
| [`/api/fleet/agents/{id}/control`](#apifleetagentsidcontrol) | POST | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/crons`](#apifleetagentsidcrons) | GET | `session` + `agent-reader` |
| [`/api/fleet/agents/{id}/crons`](#apifleetagentsidcrons) | POST | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | POST | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | PUT | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | DELETE | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/secrets`](#apifleetagentsidsecrets) | GET | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/secrets`](#apifleetagentsidsecrets) | PUT | `session` + `agent-manager` |
| [`/api/fleet/agents/{id}/secrets`](#apifleetagentsidsecrets) | DELETE | `session` + `agent-manager` |
| [`/api/fleet/containers`](#apifleetcontainers) | GET | `session` |
| [`/api/fleet/create`](#apifleetcreate) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/crons`](#apifleetcrons) | GET | `admin` |
| [`/api/fleet/crons`](#apifleetcrons) | POST | `admin` |
| [`/api/fleet/defs`](#apifleetdefs) | GET | `session` |
| [`/api/fleet/defs/{id}`](#apifleetdefsid) | PATCH | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/edit`](#apifleetdefsidedit) | POST | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/google`](#apifleetdefsidgoogle) | GET | `session` + `agent-reader` |
| [`/api/fleet/defs/{id}/google`](#apifleetdefsidgoogle) | PUT | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/google`](#apifleetdefsidgoogle) | DELETE | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/google/connect`](#apifleetdefsidgoogleconnect) | GET | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/managers`](#apifleetdefsidmanagers) | GET | `session` + `agent-reader` |
| [`/api/fleet/defs/{id}/managers`](#apifleetdefsidmanagers) | PUT | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/mcp`](#apifleetdefsidmcp) | GET | `session` + `agent-reader` |
| [`/api/fleet/defs/{id}/mcp`](#apifleetdefsidmcp) | POST | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/rotate-key`](#apifleetdefsidrotate-key) | POST | `session` + `agent-manager` |
| [`/api/fleet/defs/{id}/versions`](#apifleetdefsidversions) | GET | `session` + `agent-reader` |
| [`/api/fleet/defs/{id}/versions`](#apifleetdefsidversions) | POST | `session` + `agent-manager` |
| [`/api/fleet/endpoints`](#apifleetendpoints) | GET | `admin` |
| [`/api/fleet/endpoints`](#apifleetendpoints) | POST | `admin` |
| [`/api/fleet/endpoints/{id}`](#apifleetendpointsid) | PUT | `admin` |
| [`/api/fleet/endpoints/{id}`](#apifleetendpointsid) | DELETE | `admin` |
| [`/api/fleet/endpoints/{id}/available`](#apifleetendpointsidavailable) | GET | `admin` |
| [`/api/fleet/federate`](#apifleetfederate) | POST | `admin` |
| [`/api/fleet/hires`](#apifleethires) | GET | `session` + `perm:agents.manage` |
| [`/api/fleet/reconcile`](#apifleetreconcile) | POST | `admin` |
| [`/api/fleet/render`](#apifleetrender) | POST | `admin` |
| [`/api/fleet/resources`](#apifleetresources) | GET | `admin` |
| [`/api/fleet/roll`](#apifleetroll) | GET | `admin` |
| [`/api/fleet/roll`](#apifleetroll) | POST | `admin` |
| [`/api/fleet/seal-configs`](#apifleetseal-configs) | POST | `admin` |

## `/api/fleet`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet.rs)

> GET /api/fleet. Owned fleet ops data (agents + Talaria-native usage).
> Ops-wide detail: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/fleet/agents/{id}/control`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_control.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_control.rs)

> POST /api/fleet/agents/{id}/control. Lifecycle control for one agent —
> this agent's managers, and admins (docs/PERMISSIONS.md, "Agent managers").
>   up | stop | restart   the managed service (renders first on `up`)
>   roll                  recreate the container in place — detached
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `agent-manager` | [body](#post-apifleetagentsidcontrol-body) | `{ok, rolling} or {ok, pending, state} or {ok, warming} or {ok} or {ok, render} or {ok, removedVolume}` | 200, 202, 400, 404, 409, 500, … | audit |

### POST `/api/fleet/agents/{id}/control` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(up|stop|restart|roll|retire|unretire|delete)` |  |

## `/api/fleet/agents/{id}/crons`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_crons.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_crons.rs)

> /api/fleet/agents/{id}/crons. One agent's native Hermes cron jobs. GET →
> jobs (read from the agent's own `/api/jobs`). POST → create. Admin, or the
> owner of a personal assistant.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-reader` | — | `{jobs}` | 200, 400 | — |
| POST | `session` + `agent-manager` | [body](#post-apifleetagentsidcrons-body) | `{ok, id}` | 200, 400 | audit |

### POST `/api/fleet/agents/{id}/crons` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(1, 80)` |  |
| `schedule` | `string trimmed(1, 120)` |  |
| `prompt` | `string trimmed(1, 5000)` | The agent's own cap: Hermes's /api/jobs refuses a longer prompt (talaria_agent_crons::MAX_PROMPT_CHARS). |

## `/api/fleet/agents/{id}/crons/{jobId}`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_crons_jobid.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_crons_jobid.rs)

> /api/fleet/agents/{id}/crons/{jobId}. One cron job: DELETE → remove. POST
> { action } → pause | resume | run ("run" queues it for the next scheduler
> tick, ≤60s; a paused job is refused, because Hermes's run would resume it
> for good). PUT { name? schedule? prompt? } → edit in place. Admin or
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `agent-manager` | [body](#post-apifleetagentsidcronsjobid-body) | `{ok}` | 200, 400 | audit |
| PUT | `session` + `agent-manager` | [body](#put-apifleetagentsidcronsjobid-body) | `{ok}` | 200, 400 | audit |
| DELETE | `session` + `agent-manager` | — | `{ok}` | 200, 400 | audit |

### POST `/api/fleet/agents/{id}/crons/{jobId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(pause|resume|run)` |  |

### PUT `/api/fleet/agents/{id}/crons/{jobId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(80)` |  |
| `schedule` | `string?(120)` |  |
| `prompt` | `string?(5000)` | The agent's own cap: Hermes's /api/jobs refuses a longer prompt (talaria_agent_crons::MAX_PROMPT_CHARS). |

## `/api/fleet/agents/{id}/secrets`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_secrets.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_agents_id_secrets.rs)

> /api/fleet/agents/{id}/secrets. Per-agent secrets, write-only. GET →
> names + timestamps (never values), and for a CONFIG_* secret where the
> current config names it (usedAt). PUT { name, value } → set/replace.
> DELETE { name, removeSetting? } → remove; a CONFIG_* secret the current
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-manager` | — | `{secrets}` | 200 | — |
| PUT | `session` + `agent-manager` | [body](#put-apifleetagentsidsecrets-body) | `{ok}` | 200, 400 | — |
| DELETE | `session` + `agent-manager` | [body](#delete-apifleetagentsidsecrets-body) | `{ok}` | 200, 400, 409 | audit |

### PUT `/api/fleet/agents/{id}/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(2, 64)` |  |
| `value` | `string(1, 8192)` | value is UNtrimmed — a leading space is a legal secret character. |

### DELETE `/api/fleet/agents/{id}/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 64)` |  |
| `removeSetting` | `bool?` |  |

## `/api/fleet/containers`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_containers.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_containers.rs)

> GET /api/fleet/containers. Container reality per agent (the managed
> service). Admins and `agents.manage` see the whole fleet; a manager sees
> the departments their OWN agents run in — without it their tiles would
> render every agent as stopped, which is the one thing this read exists to
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{containers}` | 200 | — |

## `/api/fleet/create`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_create.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_create.rs)

> /api/fleet/create. POST → start HIRING a new agent. The work — create the
> def, write v1 and any starter skills, render the fleet, boot the
> container, wait out the healthcheck — is a durable `agent-hire` run, not
> this request: a boot runs to minutes on a cold pull, and a POST is a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetcreate-body) | `{ok, hire}` | 200, 400, 409, 500 | — |

### POST `/api/fleet/create` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `string(2, 30)` |  |
| `department` | `string(2, 40)` |  |
| `displayName` | `string(1, 60)` |  |
| `role` | `string? nullish(80)` |  |
| `templateId` | `uuid?` |  |
| `soul` | `string?(200000)` |  |
| `start` | `bool?` |  |

## `/api/fleet/crons`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_crons.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_crons.rs)

> /api/fleet/crons. Fleet-wide crons (admin). GET → every managed agent's
> jobs (down containers reported per-agent, not fatal). POST → create the
> same job across agents, staggered per agent when the schedule is a
> fixed-minute cron expression.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{agents}` | 200 | — |
| POST | `admin` | [body](#post-apifleetcrons-body) | `{results}` | 200, 400 | audit |

### POST `/api/fleet/crons` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentIds` | `uuid[](64)` | agentIds: min(1).max(64) — the helper carries the max; the min (a fleet cron with no agents is nothing) is checked here. |
| `name` | `string trimmed(1, 80)` |  |
| `schedule` | `string trimmed(1, 120)` |  |
| `prompt` | `string trimmed(1, 5000)` | The agent's own cap: Hermes's /api/jobs refuses a longer prompt (talaria_agent_crons::MAX_PROMPT_CHARS). |

## `/api/fleet/defs`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs.rs)

> GET /api/fleet/defs. The harness registry: agent definitions (latest
> version inline) + LLM endpoints + brain routability + toolset vocabulary.
>
> Who sees what: `agents.manage` (and admins) read the whole fleet; everyone
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{defs, endpoints, brains, canHire, toolsets}` | 200 | — |

**GET** — The roster also carries `toolsets`, the agent editor's vocabulary: `governed`, every Hermes toolset an operator may disable on an agent (21 names, alphabetical); `mandatoryDisabled`, the whole floor the renderer closes on every agent whatever its def says — `todo` and `clarify`, because Talaria's own MCP versions are the ones a person sees, plus the org-wide floor an admin sets on `PUT /api/admin/settings` (`mandatoryDisabledToolsets`) or pins with `TALARIA_MANDATORY_DISABLED_TOOLSETS`; and `orgDisabled`, that org part alone, so the editor can say an admin closed a row rather than Talaria. `governed` is the constant the `PATCH /api/fleet/defs/{id}` validator reads, so the picker cannot offer a name the write door would refuse.

## `/api/fleet/defs/{id}`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id.rs)

> /api/fleet/defs/{id}. PATCH → editable agent identity metadata (role,
> display name, send alias) plus the Developer Agent switch, the delegation
> cap, the toolsets the agent does not get, and template binds. Not
> versioned: this is identity, not config. Admin only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` + `agent-manager` | [body](#patch-apifleetdefsid-body) | `{ok}` | 200, 400, 404 | audit |

### PATCH `/api/fleet/defs/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `role` | `string? nullable(80)` |  |
| `ticketTemplateId` | `uuid? nullable` | Template overrides: uuid binds, null clears, omitted leaves unchanged. |
| `planTemplateId` | `uuid? nullable` |  |
| `developer` | `bool?` | The Developer Agent switch: on sets the agent up end to end (sandbox, Oh My Pi, Workbench tools); the roll below applies it. |
| `maxChildren` | `number?(0, 16)` | How many subagents this agent may run at once; 0 means it delegates to nobody. The ceiling is the box's, not the agent's. Applied in place: a running agent reads it on its next delegate call. |
| `pinnedSessions` | `bool?` | Pinned sessions: one Hermes session per conversation, Hermes holding the history, so its compaction and session search see the whole thread. Talaria-side only — no roll. |
| `disabledToolsets` | `string[]?(1, 40, 40)` | The Hermes toolsets this agent does NOT get — the whole list, or absent. Only governed names pass (a stranger is ignored in silence and the toolset stays open). Applied in place: a running agent reads it on its next turn. |

## `/api/fleet/defs/{id}/edit`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_edit.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_edit.rs)

> /api/fleet/defs/{id}/edit. POST → save an edit as a NEW immutable version
> (and optionally apply it to the running managed container). Admin. This is
> "versioned agent internals": nothing shifts silently — every change is a
> version you can diff and revert.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `agent-manager` | [body](#post-apifleetdefsidedit-body) | `{ok, version, created, applied, notes}` | 200, 400, 404 | audit |

### POST `/api/fleet/defs/{id}/edit` body

| field | schema | notes |
| :--- | :--- | :--- |
| `soul` | `string(0, 200000)` |  |
| `main` | `target` |  |
| `aliases` | `target_array` |  |
| `fallbacks` | `target_array` |  |
| `note` | `string?(300)` |  |
| `apply` | `bool?` | Apply the edit to the running agent now: re-render (config and soul land in place) and restart the managed container only if the edit changed something it reads only at start. |
| `compression` | `parse_compression` | The compaction knobs, and what the answer owes the caller about them. note: `compression` — `{ threshold?: number|null (0.1..1), thresholdTokens?: number|null (1000..10000000) }`; null clears a knob, absent leaves it; a threshold under 0.75 saves and the answer's `notes` says Hermes raises it for models under 512K context |

## `/api/fleet/defs/{id}/google`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_google.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_google.rs)

> /api/fleet/defs/{id}/google — who this agent acts as on Google, and the
> connect/disconnect control for a per-agent identity.
>
> GET    → the registry row (or the legacy fallback) plus connection face
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-reader` | — | `…` | 200, 404 | — |
| PUT | `session` + `agent-manager` | [body](#put-apifleetdefsidgoogle-body) | `…` | 200, 400, 403, 404 | audit |
| DELETE | `session` + `agent-manager` | — | `…` | 200, 404 | audit |

### PUT `/api/fleet/defs/{id}/google` body

| field | schema | notes |
| :--- | :--- | :--- |
| `principalKind` | `enum(owner|org|agent)` |  |

## `/api/fleet/defs/{id}/google/connect`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_google.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_google.rs)

> /api/fleet/defs/{id}/google — who this agent acts as on Google, and the
> connect/disconnect control for a per-agent identity.
>
> GET    → the registry row (or the legacy fallback) plus connection face
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-manager` | — | `…` | 302, 400, 404 | — |

## `/api/fleet/defs/{id}/managers`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_managers.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_managers.rs)

> /api/fleet/defs/{id}/managers. Who owns this agent. GET → the roster, for
> anyone who can see the agent. PUT { userIds } → replace it: this agent's
> managers and admins, because handing an agent over is itself a change to
> the agent (docs/PERMISSIONS.md, "Agent managers").
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-reader` | — | `{managers}` | 200, 404 | — |
| PUT | `session` + `agent-manager` | [body](#put-apifleetdefsidmanagers-body) | `{managers}` | 200, 400, 404 | audit |

### PUT `/api/fleet/defs/{id}/managers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userIds` | `string[](36, 36, 1, 50)` | Required array of uuids, at least one, at most fifty — the body validates before the agent lookup, as everywhere else in this family. |

## `/api/fleet/defs/{id}/mcp`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_mcp.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_mcp.rs)

> /api/fleet/defs/{id}/mcp. ONE AGENT'S VIEW OF THE ORG MCP REGISTRY.
> GET → every enabled registry server with what it means for THIS agent:
> whether the agent carries it — exactly what its rendered config lists
> (`servers_for_agent`) — and why (`via`: all-agents, assigned, team,
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-reader` | — | `{agent, canInstall, servers, legacy}` | 200, 404 | — |
| POST | `session` + `agent-manager` | [body](#post-apifleetdefsidmcp-body) | `…` | 200, 400, 404 | audit |

### POST `/api/fleet/defs/{id}/mcp` body

| field | schema | notes |
| :--- | :--- | :--- |
| `unassign` | `parse_names` |  |
| `removeLegacy` | `parse_names` | `remove` is the old spelling of `removeLegacy`; both clean raw entries. |
| `remove` | `parse_names` |  |
| `apply` | `bool?` |  |

## `/api/fleet/defs/{id}/rotate-key`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_rotate_key.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_rotate_key.rs)

> POST /api/fleet/defs/{id}/rotate-key. Revoke an agent's `tak_` credential
> and mint a fresh one — the answer to a leaked key that is not "retire the
> agent". This agent's managers (and admins). The rotation is immediate at
> the door (`agent_keys` is keyed by agent, so the old secret stops
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `agent-manager` | — | `{ok, rolling}` | 200, 404 | audit |

## `/api/fleet/defs/{id}/versions`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_versions.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_defs_id_versions.rs)

> /api/fleet/defs/{id}/versions. GET → an agent definition's full version
> history (admin). POST { revertTo } → re-publish an old version's payload
> as a NEW version (history is append-only; a revert is itself a tracked
> change). A setting the old version read from a sealed credential
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `agent-reader` | — | `{def, versions}` | 200, 404 | — |
| POST | `session` + `agent-manager` | [body](#post-apifleetdefsidversions-body) | `{ok, version, created}` | 200, 400, 404 | — |

### POST `/api/fleet/defs/{id}/versions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `revertTo` | `number()` | exclusive lower bound, so the folded helper's >= min cannot say it. |

## `/api/fleet/endpoints`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_endpoints.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_endpoints.rs)

> /api/fleet/endpoints. The model-backend registry (Models tab). GET → all
> endpoints. POST → add one.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{endpoints}` | 200 | — |
| POST | `admin` | [body](#post-apifleetendpoints-body) | `{ok, id}` | 200, 400 | audit |

### POST `/api/fleet/endpoints` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(2, 60)` |  |
| `provider` | `string(2, 40)` |  |
| `baseUrl` | `url` |  |
| `class` | `enum(local|cloud)` |  |
| `apiKeyEnv` | `nullish_key_env` | Provider-key-shaped names only (see talaria-gateway key_env_allowed) — the catalog fetch sends this var's VALUE to the endpoint's base URL. |
| `apiKey` | `string?(400)` | Raw provider API key — sealed (secretbox) server-side, never stored or returned in the clear. |
| `models` | `string[]?(1, 120, 100)` |  |

## `/api/fleet/endpoints/{id}`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_endpoints_id.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_endpoints_id.rs)

> /api/fleet/endpoints/{id}. PUT → edit an endpoint (class, pricing, model
> catalog). Removing catalog models that agents use returns 409 with the
> blast radius; retry with force:true to cascade (agents get new versions
> with the tier stripped). DELETE → remove the endpoint, same double-opt-in
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apifleetendpointsid-body) | `{needsForce, affected}` | 200, 400, 404, 409 | audit |
| DELETE | `admin` | — | `{needsForce, affected}` | 200, 400, 409 | audit |

### PUT `/api/fleet/endpoints/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `class` | `enum(local|cloud)?` |  |
| `priceInPerMtok` | `nullish_nonneg` |  |
| `priceOutPerMtok` | `nullish_nonneg` |  |
| `models` | `string[]?(1, 120, 100)` |  |
| `apiKey` | `string? nullable(400)` |  |
| `force` | `bool?` |  |

## `/api/fleet/endpoints/{id}/available`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_endpoints_id_available.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_endpoints_id_available.rs)

> GET /api/fleet/endpoints/{id}/available. What this provider actually
> offers right now (live /models call, server-side, keys never leave the
> box). Admin.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{models, catalog}` | 200, 404 | — |

## `/api/fleet/federate`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_federate.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_federate.rs)

> POST /api/fleet/federate. Federate outside agents into Talaria: read a
> Hermes-format directory and create each agent natively (Talaria def,
> fresh key + state volume, our chassis, skills copied in). One-way and
> re-runnable. Admin. A roster that would put a second agent in a department
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apifleetfederate-body) | `{result}` | 200, 400, 409 | audit |

### POST `/api/fleet/federate` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dir` | `string trimmed(1, 500)` | trim, 1..500 — a server-side path to a Hermes-format directory (admin trust model). |

## `/api/fleet/hires`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_hires.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_hires.rs)

> GET /api/fleet/hires. What the roster shows while an agent-hire run
> works: every live hire, plus the recently-finished ones long enough for
> the surface to see the transition (and a failure's sentence) before the
> row goes away.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 200 | — |

## `/api/fleet/reconcile`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_reconcile.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_reconcile.rs)

> POST /api/fleet/reconcile. Render + start every enabled managed agent
> that isn't running. One button to bring the fleet to desired state
> (drift, cold start). Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `…` | 200, 500 | audit |

## `/api/fleet/render`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_render.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_render.rs)

> POST /api/fleet/render. Render every managed agent's config + the fleet
> compose + the gateway manifest (the persona door re-reads the manifest on
> each attempt).
> Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `{result}` | 200, 500 | audit |

## `/api/fleet/resources`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_resources.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_resources.rs)

> GET /api/fleet/resources?agent=<model>&minutes=<n>. The per-agent
> container resource series the run-detail modal and the Compute panel read:
> cpu/mem/pids sampled once a minute by the agent-resource-sample job.
> Admin. `agent` narrows to one; `minutes` bounds the window (default 60,
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{host, agents}` | 200 | — |

## `/api/fleet/roll`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_roll.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_roll.rs)

> /api/fleet/roll — roll every agent container at once, in-platform. Admin.
>   POST  queue an in-place roll of every managed, enabled agent that is
>         running (`{"all": true}`: every managed enabled agent), once per
>         department — the agent that runs in its container. Answers at
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{ok, rolling, batch, rolls, crossInstance}` | 200 | — |
| POST | `admin` | [body](#post-apifleetroll-body) | `{ok, rolling, skipped, batch}` | 200, 400 | audit |

### POST `/api/fleet/roll` body

| field | schema | notes |
| :--- | :--- | :--- |
| `all` | `bool?` | Every managed enabled agent, the stopped ones brought up too. |

## `/api/fleet/seal-configs`

Source: [`api/crates/talaria-routes-fleet/src/fleet/fleet_seal_configs.rs`](../../api/crates/talaria-routes-fleet/src/fleet/fleet_seal_configs.rs)

> POST /api/fleet/seal-configs. Seal the credential literals agent configs
> stored before federation sealed them on the way in: each becomes one of
> that agent's CONFIG_* secrets and every version row keeps a handle in its
> place, rewritten in place. A running agent that gains secrets is rolled
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apifleetseal-configs-body) | `{result}` | 200, 400 | audit |

### POST `/api/fleet/seal-configs` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dryRun` | `bool?` | Default true: report what sealing would do, write nothing. |
| `dropUndeliverable` | `bool?` | Take a literal no secret can carry (multi-line, over 8 KB) out of the config instead of skipping its agent. |
| `agentId` | `uuid?` | One agent; with agentIds, added to them. Neither: every agent. |
| `agentIds` | `uuid[]?(1000)` | These agents: an apply sends the ones its dry run listed. |


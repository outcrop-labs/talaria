# API reference — fleet

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

20 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/fleet`](#apifleet) | GET | `session` + `view:/observability` |
| [`/api/fleet/agents/{id}/control`](#apifleetagentsidcontrol) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons`](#apifleetagentsidcrons) | GET | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons`](#apifleetagentsidcrons) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | PUT | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | DELETE | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/secrets`](#apifleetagentsidsecrets) | GET | `session` |
| [`/api/fleet/agents/{id}/secrets`](#apifleetagentsidsecrets) | PUT | `session` |
| [`/api/fleet/agents/{id}/secrets`](#apifleetagentsidsecrets) | DELETE | `session` |
| [`/api/fleet/containers`](#apifleetcontainers) | GET | `admin` |
| [`/api/fleet/create`](#apifleetcreate) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/crons`](#apifleetcrons) | GET | `admin` |
| [`/api/fleet/crons`](#apifleetcrons) | POST | `admin` |
| [`/api/fleet/defs`](#apifleetdefs) | GET | `session` + `perm:agents.manage` |
| [`/api/fleet/defs/{id}`](#apifleetdefsid) | PATCH | `session` + `perm:agents.manage` |
| [`/api/fleet/defs/{id}/edit`](#apifleetdefsidedit) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/defs/{id}/mcp`](#apifleetdefsidmcp) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/defs/{id}/versions`](#apifleetdefsidversions) | GET | `session` + `perm:agents.manage` |
| [`/api/fleet/defs/{id}/versions`](#apifleetdefsidversions) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/endpoints`](#apifleetendpoints) | GET | `admin` |
| [`/api/fleet/endpoints`](#apifleetendpoints) | POST | `admin` |
| [`/api/fleet/endpoints/{id}`](#apifleetendpointsid) | PUT | `admin` |
| [`/api/fleet/endpoints/{id}`](#apifleetendpointsid) | DELETE | `admin` |
| [`/api/fleet/endpoints/{id}/available`](#apifleetendpointsidavailable) | GET | `admin` |
| [`/api/fleet/federate`](#apifleetfederate) | POST | `admin` |
| [`/api/fleet/hires`](#apifleethires) | GET | `session` + `perm:agents.manage` |
| [`/api/fleet/reconcile`](#apifleetreconcile) | POST | `admin` |
| [`/api/fleet/render`](#apifleetrender) | POST | `admin` |

## `/api/fleet`

Source: [`api/src/routes/fleet/fleet.rs`](../../api/src/routes/fleet/fleet.rs)

> GET /api/fleet. Owned fleet ops data (agents + Talaria-native usage).
> Ops-wide detail: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/fleet/agents/{id}/control`

Source: [`api/src/routes/fleet/fleet_agents_id_control.rs`](../../api/src/routes/fleet/fleet_agents_id_control.rs)

> POST /api/fleet/agents/{id}/control. Lifecycle control for one agent
> (admin; owners of a personal assistant may up/stop/restart their own).
>   up | stop | restart   the managed service (renders first on `up`)
>   roll                  zero-downtime replacement (admin) — detached
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetagentsidcontrol-body) | `{ok}` | 200, 400, 403, 404, 500 | audit |

### POST `/api/fleet/agents/{id}/control` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(up|stop|restart|roll|retire|unretire|delete)` |  |

## `/api/fleet/agents/{id}/crons`

Source: [`api/src/routes/fleet/fleet_agents_id_crons.rs`](../../api/src/routes/fleet/fleet_agents_id_crons.rs)

> /api/fleet/agents/{id}/crons. One agent's native Hermes cron jobs. GET →
> jobs (read from the container's jobs.json). POST → create. Admin, or the
> owner of a personal assistant.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{jobs}` | 200, 400, 403 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetagentsidcrons-body) | `{ok, id}` | 200, 400, 403 | audit |

### POST `/api/fleet/agents/{id}/crons` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(1, 80)` |  |
| `schedule` | `string trimmed(1, 120)` |  |
| `prompt` | `string trimmed(1, 20000)` |  |

## `/api/fleet/agents/{id}/crons/{jobId}`

Source: [`api/src/routes/fleet/fleet_agents_id_crons_jobid.rs`](../../api/src/routes/fleet/fleet_agents_id_crons_jobid.rs)

> /api/fleet/agents/{id}/crons/{jobId}. One cron job: DELETE → remove. POST
> { action } → pause | resume | run ("run" queues it for the next scheduler
> tick, ≤60s). PUT { name? schedule? prompt? } → edit in place. Admin or
> owner.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetagentsidcronsjobid-body) | `{ok}` | 200, 400, 403 | audit |
| PUT | `session` + `perm:agents.manage` | [body](#put-apifleetagentsidcronsjobid-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` + `perm:agents.manage` | — | `{ok}` | 200, 400, 403 | audit |

### POST `/api/fleet/agents/{id}/crons/{jobId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(pause|resume|run)` |  |

### PUT `/api/fleet/agents/{id}/crons/{jobId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(80)` |  |
| `schedule` | `string?(120)` |  |
| `prompt` | `string?(20000)` |  |

## `/api/fleet/agents/{id}/secrets`

Source: [`api/src/routes/fleet/fleet_agents_id_secrets.rs`](../../api/src/routes/fleet/fleet_agents_id_secrets.rs)

> /api/fleet/agents/{id}/secrets. Per-agent secrets, write-only. GET →
> names + timestamps (never values). PUT { name, value } → set/replace.
> DELETE { name } → remove. Admin, or the owner of a personal assistant.
> Takes effect on the next start from Talaria. Every write audits — secret
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{secrets}` | 200, 403 | — |
| PUT | `session` | [body](#put-apifleetagentsidsecrets-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | [body](#delete-apifleetagentsidsecrets-body) | `{ok}` | 200, 400, 403 | audit |

### PUT `/api/fleet/agents/{id}/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(2, 64)` |  |
| `value` | `string(1, 8192)` | value is UNtrimmed — a leading space is a legal secret character. |

### DELETE `/api/fleet/agents/{id}/secrets` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/fleet/containers`

Source: [`api/src/routes/fleet/fleet_containers.rs`](../../api/src/routes/fleet/fleet_containers.rs)

> GET /api/fleet/containers. Container reality per agent (the managed
> service), admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{containers}` | 200 | — |

## `/api/fleet/create`

Source: [`api/src/routes/fleet/fleet_create.rs`](../../api/src/routes/fleet/fleet_create.rs)

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

Source: [`api/src/routes/fleet/fleet_crons.rs`](../../api/src/routes/fleet/fleet_crons.rs)

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
| `prompt` | `string trimmed(1, 20000)` |  |

## `/api/fleet/defs`

Source: [`api/src/routes/fleet/fleet_defs.rs`](../../api/src/routes/fleet/fleet_defs.rs)

> GET /api/fleet/defs. The harness registry: agent definitions (latest
> version inline) + LLM endpoints + brain routability. Admins only — the
> config surface includes infra layout.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{defs, endpoints, brains}` | 200 | — |

## `/api/fleet/defs/{id}`

Source: [`api/src/routes/fleet/fleet_defs_id.rs`](../../api/src/routes/fleet/fleet_defs_id.rs)

> /api/fleet/defs/{id}. PATCH → editable agent identity metadata (role,
> display name, send alias) plus the workbench and template binds. Not
> versioned — this is identity, not config. Admin only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` + `perm:agents.manage` | [body](#patch-apifleetdefsid-body) | `{ok}` | 200, 400, 404 | audit |

### PATCH `/api/fleet/defs/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `role` | `string? nullable(80)` |  |
| `ticketTemplateId` | `uuid? nullable` | Template overrides: uuid binds, null clears, omitted leaves unchanged. |
| `planTemplateId` | `uuid? nullable` |  |
| `workbench` | `enum(off|auto|on)?` |  |
| `workbenchProfile` | `string? nullable(40)` |  |
| `workbenchHarness` | `string? nullable(40)` |  |

## `/api/fleet/defs/{id}/edit`

Source: [`api/src/routes/fleet/fleet_defs_id_edit.rs`](../../api/src/routes/fleet/fleet_defs_id_edit.rs)

> /api/fleet/defs/{id}/edit. POST → save an edit as a NEW immutable version
> (and optionally apply it to the running managed container). Admin. This is
> "versioned agent internals": nothing shifts silently — every change is a
> version you can diff and revert.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetdefsidedit-body) | `{ok, version, created, applied}` | 200, 400, 404 | audit |

### POST `/api/fleet/defs/{id}/edit` body

| field | schema | notes |
| :--- | :--- | :--- |
| `soul` | `string(0, 200000)` |  |
| `main` | `target` |  |
| `aliases` | `target_array` |  |
| `fallbacks` | `target_array` |  |
| `note` | `string?(300)` |  |
| `apply` | `bool?` | Re-render + restart the managed container so the edit takes effect now. |

## `/api/fleet/defs/{id}/mcp`

Source: [`api/src/routes/fleet/fleet_defs_id_mcp.rs`](../../api/src/routes/fleet/fleet_defs_id_mcp.rs)

> /api/fleet/defs/{id}/mcp. POST → add/remove MCP servers on an agent as a
> NEW config version (same versioned-internals contract as model edits),
> optionally applied live.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetdefsidmcp-body) | `{ok, version, created, applied}` | 200, 400, 404 | audit |

### POST `/api/fleet/defs/{id}/mcp` body

| field | schema | notes |
| :--- | :--- | :--- |
| `apply` | `bool?` |  |

## `/api/fleet/defs/{id}/versions`

Source: [`api/src/routes/fleet/fleet_defs_id_versions.rs`](../../api/src/routes/fleet/fleet_defs_id_versions.rs)

> /api/fleet/defs/{id}/versions. GET → an agent definition's full version
> history (admin). POST { revertTo } → re-publish an old version's payload
> as a NEW version (history is append-only; a revert is itself a tracked
> change).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{def, versions}` | 200, 404 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetdefsidversions-body) | `{ok, version, created}` | 200, 400, 404 | — |

### POST `/api/fleet/defs/{id}/versions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `revertTo` | `number()` | exclusive lower bound, so the folded helper's >= min cannot say it. |

## `/api/fleet/endpoints`

Source: [`api/src/routes/fleet/fleet_endpoints.rs`](../../api/src/routes/fleet/fleet_endpoints.rs)

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
| `apiKeyEnv` | `nullish_key_env` | Provider-key-shaped names only (see provider-catalog KEY_ENV_RE) — the catalog fetch sends this var's VALUE to the endpoint's base URL. |
| `apiKey` | `string?(400)` | Raw provider API key — sealed (secretbox) server-side, never stored or returned in the clear. |
| `models` | `string[]?(1, 120, 100)` |  |

## `/api/fleet/endpoints/{id}`

Source: [`api/src/routes/fleet/fleet_endpoints_id.rs`](../../api/src/routes/fleet/fleet_endpoints_id.rs)

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

Source: [`api/src/routes/fleet/fleet_endpoints_id_available.rs`](../../api/src/routes/fleet/fleet_endpoints_id_available.rs)

> GET /api/fleet/endpoints/{id}/available. What this provider actually
> offers right now (live /models call, server-side, keys never leave the
> box). Admin.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{models, catalog}` | 200, 404 | — |

## `/api/fleet/federate`

Source: [`api/src/routes/fleet/fleet_federate.rs`](../../api/src/routes/fleet/fleet_federate.rs)

> POST /api/fleet/federate. Federate outside agents into Talaria: read a
> Hermes-format directory and create each agent natively (Talaria def,
> fresh key + state volume, our chassis, skills copied in). One-way and
> re-runnable. Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apifleetfederate-body) | `{result}` | 200, 400 | audit |

### POST `/api/fleet/federate` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dir` | `string trimmed(1, 500)` | trim, 1..500 — a server-side path to a Hermes-format directory (admin trust model). |

## `/api/fleet/hires`

Source: [`api/src/routes/fleet/fleet_hires.rs`](../../api/src/routes/fleet/fleet_hires.rs)

> GET /api/fleet/hires. What the roster shows while an agent-hire run
> works: every live hire, plus the recently-finished ones long enough for
> the surface to see the transition (and a failure's sentence) before the
> row goes away.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 200 | — |

## `/api/fleet/reconcile`

Source: [`api/src/routes/fleet/fleet_reconcile.rs`](../../api/src/routes/fleet/fleet_reconcile.rs)

> POST /api/fleet/reconcile. Render + start every enabled managed agent
> that isn't running. One button to bring the fleet to desired state
> (drift, cold start). Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `…` | 200, 500 | audit |

## `/api/fleet/render`

Source: [`api/src/routes/fleet/fleet_render.rs`](../../api/src/routes/fleet/fleet_render.rs)

> POST /api/fleet/render. Render every managed agent's config + the fleet
> compose + the gateway manifest (the bridge hot-reloads the manifest).
> Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `{result}` | 200, 500 | audit |


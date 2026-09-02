# API reference — fleet

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

20 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/fleet`](#apifleet) | GET | `session` + `view:/observability` |
| [`/api/fleet/agents/{id}/control`](#apifleetagentsidcontrol) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons`](#apifleetagentsidcrons) | GET | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons`](#apifleetagentsidcrons) | POST | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | DELETE | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | PUT | `session` + `perm:agents.manage` |
| [`/api/fleet/agents/{id}/crons/{jobId}`](#apifleetagentsidcronsjobid) | POST | `session` + `perm:agents.manage` |
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

Source: [`api/src/routes/fleet.rs`](../../api/src/routes/fleet.rs)

> GET /api/fleet → owned fleet ops data (agents + Talaria-native usage).
> Ops-wide detail: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/fleet/agents/{id}/control`

Source: [`api/src/routes/fleet_agents_id_control.rs`](../../api/src/routes/fleet_agents_id_control.rs)

> POST { action } → lifecycle control for one agent (admin; owners of a
> personal assistant may up/stop/restart their own).
>   up | stop | restart   the managed service (renders first on `up`)
>   roll                  zero-downtime replacement (admin) — detached
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetagentsidcontrol-body) | `{ok, warming}` | 200, 400, 403, 404, 500 | audit |

### POST `/api/fleet/agents/{id}/control` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.enum(['up', 'stop', 'restart', 'roll', 'retire', 'unretire', 'delete'])` |  |

## `/api/fleet/agents/{id}/crons`

Source: [`api/src/routes/fleet_agents_id_crons.rs`](../../api/src/routes/fleet_agents_id_crons.rs)

> One agent's native Hermes cron jobs. GET → jobs (read from the container's
> jobs.json). POST → create. Admin, or the owner of a personal assistant.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{jobs}` | 200, 400, 403 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetagentsidcrons-body) | `{ok}` | 200, 400, 403 | audit |

### POST `/api/fleet/agents/{id}/crons` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().trim().min(1).max(80)` |  |
| `schedule` | `z.string().trim().min(1).max(120)` |  |
| `prompt` | `z.string().trim().min(1).max(20_000)` |  |

## `/api/fleet/agents/{id}/crons/{jobId}`

Source: [`api/src/routes/fleet_agents_id_crons_jobid.rs`](../../api/src/routes/fleet_agents_id_crons_jobid.rs)

> One cron job: DELETE → remove. POST { action } → pause | resume | run
> ("run" queues it for the next scheduler tick, ≤60s). PUT { name? schedule?
> prompt? } → edit in place. Admin or owner.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| DELETE | `session` + `perm:agents.manage` | — | `{ok}` | 200, 400, 403 | audit |
| PUT | `session` + `perm:agents.manage` | [body](#put-apifleetagentsidcronsjobid-body) | `{ok}` | 200, 400, 403 | audit |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetagentsidcronsjobid-body) | `{ok}` | 200, 400, 403 | audit |

### PUT `/api/fleet/agents/{id}/crons/{jobId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80).optional()` |  |
| `schedule` | `z.string().min(1).max(120).optional()` |  |
| `prompt` | `z.string().min(1).max(20_000).optional()` |  |

### POST `/api/fleet/agents/{id}/crons/{jobId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `z.enum(['pause', 'resume', 'run'])` |  |

## `/api/fleet/agents/{id}/secrets`

Source: [`api/src/routes/fleet_agents_id_secrets.rs`](../../api/src/routes/fleet_agents_id_secrets.rs)

> Per-agent secrets, write-only. GET → names + timestamps (never values).
> PUT { name, value } → set/replace. DELETE { name } → remove. Admin, or the
> owner of a personal assistant. Takes effect on the next start from Talaria.
> Every write audits — secret NAMES only, never values.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{secrets}` | 200, 403 | — |
| PUT | `session` | [body](#put-apifleetagentsidsecrets-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | [body](#delete-apifleetagentsidsecrets-body) | `{ok}` | 200, 400, 403 | audit |

### PUT `/api/fleet/agents/{id}/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().trim().min(2).max(64)` |  |
| `value` | `z.string().min(1).max(8192)` |  |

### DELETE `/api/fleet/agents/{id}/secrets` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(64)` |  |

## `/api/fleet/containers`

Source: [`api/src/routes/fleet_containers.rs`](../../api/src/routes/fleet_containers.rs)

> GET → container reality per agent (the managed service), admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{containers}` | 200 | — |

## `/api/fleet/create`

Source: [`api/src/routes/fleet_create.rs`](../../api/src/routes/fleet_create.rs)

> POST → start HIRING a new agent. The work — create the def, write v1 and
> any starter skills, render the fleet, boot the container, wait out the
> healthcheck — is a durable `agent-hire` run, not this request: a boot runs
> to minutes on a cold pull, and a POST is a promise to stay on the line the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetcreate-body) | `{ok, hire}` | 200, 409 | — |

### POST `/api/fleet/create` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `z.string().min(2).max(30)` |  |
| `department` | `z.string().min(2).max(40)` |  |
| `displayName` | `z.string().min(1).max(60)` |  |
| `role` | `z.string().max(80).nullish()` |  |
| `templateId` | `Uuid.optional()` |  |
| `soul` | `z.string().max(200_000).optional()` |  |
| `skills` | `z.array(z.object({ name: z.string().regex(/^[a-z0-9][a-z0-9._-]*$/), content: z.string().max(100_000) })).max(5).optional()` |  |
| `start` | `z.boolean().optional()` |  |

## `/api/fleet/crons`

Source: [`api/src/routes/fleet_crons.rs`](../../api/src/routes/fleet_crons.rs)

> Fleet-wide crons (admin). GET → every managed agent's jobs (down containers
> reported per-agent, not fatal). POST → create the same job across agents,
> staggered per agent when the schedule is a fixed-minute cron expression.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{agents}` | 200 | — |
| POST | `admin` | [body](#post-apifleetcrons-body) | `…` | 200 | audit |

### POST `/api/fleet/crons` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentIds` | `z.array(Uuid).min(1).max(64)` |  |
| `name` | `z.string().trim().min(1).max(80)` |  |
| `schedule` | `z.string().trim().min(1).max(120)` |  |
| `prompt` | `z.string().trim().min(1).max(20_000)` |  |
| `staggerMinutes` | `z.number().int().min(0).max(30).optional()` |  |

## `/api/fleet/defs`

Source: [`api/src/routes/fleet_defs.rs`](../../api/src/routes/fleet_defs.rs)

> The harness registry. GET → agent definitions (latest version inline) +
> LLM endpoints. Admins only — the config surface includes infra layout.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{defs, endpoints, brains}` | 200 | — |

## `/api/fleet/defs/{id}`

Source: [`api/src/routes/fleet_defs_id.rs`](../../api/src/routes/fleet_defs_id.rs)

> PATCH → editable agent identity metadata (role, display name). Not versioned
> — this is identity, not config. Admin only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` + `perm:agents.manage` | [body](#patch-apifleetdefsid-body) | `{ok}` | 200, 404 | audit |

### PATCH `/api/fleet/defs/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `role` | `z.string().max(80).nullish()` |  |
| `displayName` | `z.string().min(1).max(80).optional()` |  |
| `emailAlias` | `z.string().trim().max(320).refine((v) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v), 'not an email address').nullish()` |  |
| `ticketTemplateId` | `Uuid.nullable().optional()` |  |
| `planTemplateId` | `Uuid.nullable().optional()` |  |
| `workbench` | `z.enum(['off', 'auto', 'on']).optional()` |  |
| `workbenchProfile` | `z.string().max(40).nullable().optional()` |  |
| `workbenchHarness` | `z.string().max(40).nullable().optional()` |  |
| `workbenchModels` | `z.object({ light: z.string().max(200).nullable().optional(), standard: z.string().max(200).nullable().optional(), heavy: z.string().max(200…` |  |

## `/api/fleet/defs/{id}/edit`

Source: [`api/src/routes/fleet_defs_id_edit.rs`](../../api/src/routes/fleet_defs_id_edit.rs)

> POST → save an edit as a NEW immutable version (and optionally apply it to
> the running managed container). Admin. This is "versioned agent internals":
> nothing shifts silently — every change is a version you can diff and revert.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetdefsidedit-body) | `{ok, applied, warning}` | 200, 400, 404 | audit |

### POST `/api/fleet/defs/{id}/edit` body

| field | schema | notes |
| :--- | :--- | :--- |
| `soul` | `z.string().max(200_000)` |  |
| `main` | `Target` |  |
| `aliases` | `z.array(Target.extend({ name: z.string().min(1).max(60) })).max(20)` |  |
| `fallbacks` | `z.array(Target).max(10)` |  |
| `note` | `z.string().max(300).optional()` |  |
| `apply` | `z.boolean().optional()` |  |

## `/api/fleet/defs/{id}/mcp`

Source: [`api/src/routes/fleet_defs_id_mcp.rs`](../../api/src/routes/fleet_defs_id_mcp.rs)

> POST → add/remove MCP servers on an agent as a NEW config version (same
> versioned-internals contract as model edits), optionally applied live.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetdefsidmcp-body) | `{ok, applied, warning}` | 200, 400, 404 | audit |

### POST `/api/fleet/defs/{id}/mcp` body

| field | schema | notes |
| :--- | :--- | :--- |
| `add` | `z.array(z.object({ name: z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(60), url: z.string().url().max(300), timeout: z.number().int().posit…` |  |
| `remove` | `z.array(z.string().max(60)).max(20).default([])` |  |
| `apply` | `z.boolean().optional()` |  |

## `/api/fleet/defs/{id}/versions`

Source: [`api/src/routes/fleet_defs_id_versions.rs`](../../api/src/routes/fleet_defs_id_versions.rs)

> GET → an agent definition's full version history (admin).
> POST { revertTo } → re-publish an old version's payload as a NEW version
> (history is append-only; a revert is itself a tracked change).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{versions}` | 200, 404 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apifleetdefsidversions-body) | `{ok}` | 200, 404 | — |

### POST `/api/fleet/defs/{id}/versions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `revertTo` | `z.number().int().positive()` |  |

## `/api/fleet/endpoints`

Source: [`api/src/routes/fleet_endpoints.rs`](../../api/src/routes/fleet_endpoints.rs)

> The model-backend registry (Models tab). GET → all endpoints. POST → add one.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{endpoints}` | 200 | — |
| POST | `admin` | [body](#post-apifleetendpoints-body) | `{ok}` | 200, 400 | audit |

### POST `/api/fleet/endpoints` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(2).max(60)` |  |
| `provider` | `z.string().min(2).max(40)` |  |
| `baseUrl` | `z.string().url().max(300).nullish()` |  |
| `class` | `z.enum(['local', 'cloud'])` |  |
| `apiKeyEnv` | `z.string().regex(/^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$/).max(80).nullish()` |  |
| `apiKey` | `z.string().max(400).nullish()` |  |
| `models` | `z.array(z.string().min(1).max(120)).max(100).optional()` |  |
| `modelPrices` | `z.record(z.string().max(120), z.object({ in: z.number().nonnegative().optional(), out: z.number().nonnegative().optional() })).optional()` |  |

## `/api/fleet/endpoints/{id}`

Source: [`api/src/routes/fleet_endpoints_id.rs`](../../api/src/routes/fleet_endpoints_id.rs)

> PUT → edit an endpoint (class, pricing, model catalog). Removing catalog
> models that agents use returns 409 with the blast radius; retry with
> force:true to cascade (agents get new versions with the tier stripped).
> DELETE → remove the endpoint, same double-opt-in flow (?force=1).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apifleetendpointsid-body) | `{needsForce, affected}` | 200, 400, 404, 409 | audit |
| DELETE | `admin` | — | `{ok}` | 200, 400, 409 | audit |

### PUT `/api/fleet/endpoints/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `class` | `z.enum(['local', 'cloud']).optional()` |  |
| `priceInPerMtok` | `z.number().nonnegative().nullish()` |  |
| `priceOutPerMtok` | `z.number().nonnegative().nullish()` |  |
| `models` | `z.array(z.string().min(1).max(120)).max(100).optional()` |  |
| `modelPrices` | `z.record(z.string().max(120), z.object({ in: z.number().nonnegative().optional(), out: z.number().nonnegative().optional() })).optional()` |  |
| `modelEfforts` | `z.record(z.string().max(120), z.array(z.string().min(1).max(24)).min(1).max(12)).optional()` |  |
| `requestDefaults` | `z.record(z.string().max(120), z.unknown()).optional()` |  |
| `apiKey` | `z.string().max(400).nullish()` |  |
| `force` | `z.boolean().optional()` |  |

## `/api/fleet/endpoints/{id}/available`

Source: [`api/src/routes/fleet_endpoints_id_available.rs`](../../api/src/routes/fleet_endpoints_id_available.rs)

> GET → what this provider actually offers right now (live /models call,
> server-side, keys never leave the box). Admin.
>
> THE CALL IS ALREADY BEING MADE, so it also refreshes the stored catalog: an
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{models, catalog}` | 200, 404 | — |

## `/api/fleet/federate`

Source: [`api/src/routes/fleet_federate.rs`](../../api/src/routes/fleet_federate.rs)

> POST → federate outside agents into Talaria: read a Hermes-format directory
> and create each agent natively (Talaria def, fresh key + state volume, our
> chassis, skills copied in). One-way and re-runnable. Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apifleetfederate-body) | `…` | 200 | audit |

### POST `/api/fleet/federate` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dir` | `z.string().trim().min(1).max(500)` |  |

## `/api/fleet/hires`

Source: [`api/src/routes/fleet_hires.rs`](../../api/src/routes/fleet_hires.rs)

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 200 | — |

## `/api/fleet/reconcile`

Source: [`api/src/routes/fleet_reconcile.rs`](../../api/src/routes/fleet_reconcile.rs)

> POST → render + start every enabled managed agent that isn't running. One
> button to bring the fleet to desired state (drift, cold start). Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `…` | 200, 500 | audit |

## `/api/fleet/render`

Source: [`api/src/routes/fleet_render.rs`](../../api/src/routes/fleet_render.rs)

> POST → render every managed agent's config + the fleet compose + the gateway
> manifest (the bridge hot-reloads the manifest). Admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `…` | 200, 500 | audit |


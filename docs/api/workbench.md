# API reference — workbench

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

7 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/workbench`](#apiworkbench) | GET | `session` + `perm:agents.manage` |
| [`/api/workbench`](#apiworkbench) | PUT | `session` + `perm:agents.manage` |
| [`/api/workbench/flow`](#apiworkbenchflow) | GET | `session` + `perm:agents.manage` |
| [`/api/workbench/flow`](#apiworkbenchflow) | PUT | `session` + `perm:agents.manage` |
| [`/api/workbench/github`](#apiworkbenchgithub) | GET | `admin` |
| [`/api/workbench/github`](#apiworkbenchgithub) | PUT | `admin` |
| [`/api/workbench/github`](#apiworkbenchgithub) | DELETE | `admin` |
| [`/api/workbench/harnesses`](#apiworkbenchharnesses) | GET | `session` |
| [`/api/workbench/harnesses`](#apiworkbenchharnesses) | PUT | `session` + `perm:agents.manage` |
| [`/api/workbench/harnesses`](#apiworkbenchharnesses) | DELETE | `session` + `perm:agents.manage` |
| [`/api/workbench/jobs`](#apiworkbenchjobs) | GET | `session` |
| [`/api/workbench/jobs`](#apiworkbenchjobs) | PUT | `session` |
| [`/api/workbench/repo-requests`](#apiworkbenchrepo-requests) | GET | `admin` |
| [`/api/workbench/repo-requests`](#apiworkbenchrepo-requests) | PUT | `admin` |
| [`/api/workbench/repos/{agentId}`](#apiworkbenchreposagentid) | GET | `session` + `perm:agents.manage` |
| [`/api/workbench/repos/{agentId}`](#apiworkbenchreposagentid) | PUT | `session` + `perm:agents.manage` |

## `/api/workbench`

Source: [`ui/src/routes/api/workbench.ts`](../../ui/src/routes/api/workbench.ts)

> Workbench profiles — the role-agnostic sandbox registry ('dev' seeded;
> designer/data/etc ride the same table). GET → any member (the Studio and
> agent views show attachment state); PUT → agents.manage, except the
> infrastructure fields, which are admin-only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{profiles}` | 200 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbench-body) | `{ok}` | 200, 400, 403, 404 | audit |

### PUT `/api/workbench` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `z.string().min(1).max(40)` |  |
| `name` | `z.string().min(1).max(80).optional()` |  |
| `description` | `z.string().max(500).optional()` |  |
| `env` | `z.record(z.string(), z.string().max(500)).optional()` |  |
| `harnesses` | `z.array(z.string().max(40)).max(20).optional()` |  |
| `autoAttach` | `z.object({ departments: z.array(z.string().max(60)).max(20).optional(), roles: z.array(z.string().max(60)).max(20).optional() }).optional()` |  |
| `enabled` | `z.boolean().optional()` |  |
| `image` | `z.string().max(200).optional()` |  |
| `mounts` | `z.array(z.string().max(300)).max(20).optional()` |  |

## `/api/workbench/flow`

Source: [`ui/src/routes/api/workbench.flow.ts`](../../ui/src/routes/api/workbench.flow.ts)

> Per-repo git flow (PR base + optional testing branch). GET → configured
> flows + the reachable pool; PUT → set one repo's flow. agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 200 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchflow-body) | `{flows}` | 200 | — |

### PUT `/api/workbench/flow` body

| field | schema | notes |
| :--- | :--- | :--- |
| `repo` | `z.string().min(3).max(200)` |  |
| `baseBranch` | `z.string().max(100).nullable().optional()` |  |
| `testingBranch` | `z.string().max(100).nullable().optional()` |  |

## `/api/workbench/github`

Source: [`ui/src/routes/api/workbench.github.ts`](../../ui/src/routes/api/workbench.github.ts)

> The Workbench's GitHub connection. Deliberately requireAdmin (not
> agents.manage): this holds ORG CREDENTIALS (PAT / App private key) — a
> grantable permission shouldn't reach them. GET → live-verified redacted
> status (+ ?installations=1 lists where the App is installed, the easy-setup
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{installations}` | 200 + varies | — |
| PUT | `admin` | [body](#put-apiworkbenchgithub-body) | `{status}` | 200 + varies | audit |
| DELETE | `admin` | — | `{ok}` | 200 | audit |

### PUT `/api/workbench/github` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `z.enum(['app', 'pat']).nullable().optional()` |  |
| `pat` | `z.object({ token: z.string().max(400).nullable().optional() }).optional()` |  |
| `app` | `z.object({ appId: z.string().max(40).optional(), installationIds: z.array(z.string().max(40)).max(20).optional(), privateKey: z.string().ma…` |  |
| `repoCreationOrgs` | `z.array(z.string().min(1).max(100)).max(10).optional()` |  |

## `/api/workbench/harnesses`

Source: [`ui/src/routes/api/workbench.harnesses.ts`](../../ui/src/routes/api/workbench.harnesses.ts)

> The harness registry. GET → merged definitions with sources (any member —
> grounds the per-agent dropdowns). PUT → register/replace a CUSTOM
> definition (declarative JSON, no code); DELETE ?slug= removes one.
> Builtin/app-shipped entries can be shadowed by slug but never deleted.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{harnesses}` | 200 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchharnesses-body) | `{harnesses}` | 200 | — |
| DELETE | `session` + `perm:agents.manage` | — | `{harnesses}` | 200, 400 | — |

### PUT `/api/workbench/harnesses` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `z.string().regex(/^[a-z0-9][a-z0-9-]*$/).max(40)` |  |
| `label` | `z.string().min(1).max(60)` |  |
| `description` | `z.string().max(300).optional()` |  |
| `auth` | `z.union([z.literal('gateway'), z.object({ provider: z.string().min(1).max(40), envVar: z.string().min(1).max(60) })])` |  |
| `env` | `z.record(z.string().max(60), z.string().max(300)).optional()` |  |
| `modelPrefix` | `z.string().max(40).optional()` |  |
| `invoke` | `z.string().min(1).max(500)` |  |
| `jsonInvoke` | `z.string().max(500).optional()` |  |
| `mcpServe` | `z.object({ command: z.string().min(1).max(120), args: z.array(z.string().max(120)).max(10) }).optional()` |  |
| `mcpConfig` | `z.object({ format: z.enum(['claude-json', 'opencode-json']), filename: z.string().regex(/^[a-z0-9][a-z0-9.-]*$/).max(60) }).optional()` |  |
| `guide` | `z.string().min(1).max(2000)` |  |
| `install` | `z.object({ npm: z.array(z.string().max(120)).max(10).optional(), commands: z.array(z.string().max(300)).max(10).optional(), notes: z.string…` |  |

## `/api/workbench/jobs`

Source: [`ui/src/routes/api/workbench.jobs.ts`](../../ui/src/routes/api/workbench.jobs.ts)

> Workbench jobs from the human side. GET ?taskId= → the ticket's jobs (board
> members — this is how the plan-approval gate and PR links surface on the
> ticket). PUT → approve / reject an awaiting job (board editors; rejection
> abandons with the reason in the ticket's audit trail).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{jobs}` | 200, 400, 403, 404 | — |
| PUT | `session` | [body](#put-apiworkbenchjobs-body) | `{ok}` | 200, 400, 403, 404 + varies | — |

### PUT `/api/workbench/jobs` body

| field | schema | notes |
| :--- | :--- | :--- |
| `jobId` | `Uuid` |  |
| `action` | `z.enum(['approve', 'reject', 'merge_testing'])` |  |
| `note` | `z.string().max(500).optional()` |  |

## `/api/workbench/repo-requests`

Source: [`ui/src/routes/api/workbench.repo-requests.ts`](../../ui/src/routes/api/workbench.repo-requests.ts)

> Agent repo-creation requests. GET → pending queue; PUT → approve (creates
> the repo via the App, auto-grants it to the requester) or reject. Admin —
> approval mints real org resources.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `…` | 200 | — |
| PUT | `admin` | [body](#put-apiworkbenchrepo-requests-body) | `{ok}` | 200, 400, 404 | audit |

### PUT `/api/workbench/repo-requests` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `Uuid` |  |
| `action` | `z.enum(['approve', 'reject'])` |  |

## `/api/workbench/repos/{agentId}`

Source: [`ui/src/routes/api/workbench.repos.$agentId.ts`](../../ui/src/routes/api/workbench.repos.$agentId.ts)

> Per-agent workbench repo grants — explicit, like MCP assignment. GET →
> the connection's reachable pool + this agent's grants; PUT → replace the
> grant set (validated against the pool). agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `…` | 200, 404 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchreposagentid-body) | `{granted}` | 200, 404 | — |

### PUT `/api/workbench/repos/{agentId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `repos` | `z.array(z.string().max(200)).max(100)` |  |


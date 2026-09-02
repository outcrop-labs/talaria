# API reference — workbench

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
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

Source: [`api/src/routes/workbench/workbench.rs`](../../api/src/routes/workbench/workbench.rs)

> /api/workbench. Workbench profiles: the role-agnostic sandbox registry
> ('dev' seeded; designer/data ride the same table). GET → any member (env
> values masked — they are the documented home for scoped credentials);
> PUT → agents.manage, except the infrastructure fields, which are
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{profiles}` | 200 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbench-body) | `{ok}` | 200, 400, 403, 404 | audit |

### PUT `/api/workbench` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `string(1, 40)` | Patch — schema order (the audit trail's `after` rides it). |
| `name` | `string?(80)` |  |
| `description` | `string?(500)` |  |
| `env` | `optional_env` |  |
| `harnesses` | `string[]?(0, 40, 20)` |  |
| `autoAttach` | `optional_auto_attach` |  |
| `enabled` | `bool?` |  |
| `image` | `string?(200)` | ── admin-only below: these two reach the host, not just the sandbox ── |
| `mounts` | `string[]?(0, 300, 20)` |  |

## `/api/workbench/flow`

Source: [`api/src/routes/workbench/workbench_flow.rs`](../../api/src/routes/workbench/workbench_flow.rs)

> /api/workbench/flow. Per-repo git flow (PR base + optional testing
> branch). GET → configured flows + the reachable pool; PUT → set one
> repo's flow. agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{flows, repos}` | 200 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchflow-body) | `{flows}` | 200, 400 | — |

### PUT `/api/workbench/flow` body

| field | schema | notes |
| :--- | :--- | :--- |
| `repo` | `string(3, 200)` |  |

## `/api/workbench/github`

Source: [`api/src/routes/workbench/workbench_github.rs`](../../api/src/routes/workbench/workbench_github.rs)

> /api/workbench/github. The Workbench's GitHub connection. Deliberately
> requireAdmin (not agents.manage): this holds ORG CREDENTIALS (PAT / App
> private key) — a grantable permission shouldn't reach them. GET →
> live-verified redacted status (+ ?installations=… lists where the App is
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{status}` | 200 | — |
| PUT | `admin` | [body](#put-apiworkbenchgithub-body) | `{status}` | 200, 400 | audit |
| DELETE | `admin` | — | `{ok}` | 200 | audit |

### PUT `/api/workbench/github` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `enum(app|pat)? nullable` |  |
| `repoCreationOrgs` | `string[]?(1, 100, 10)` |  |

## `/api/workbench/harnesses`

Source: [`api/src/routes/workbench/workbench_harnesses.rs`](../../api/src/routes/workbench/workbench_harnesses.rs)

> /api/workbench/harnesses. The harness registry. GET → merged definitions
> with sources (any member — grounds the per-agent dropdowns); PUT →
> register/replace a CUSTOM definition (declarative JSON, no code);
> DELETE ?slug= removes one. Builtin/app-shipped entries can be shadowed by
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchharnesses-body) | `…` | 200, 400 | — |
| DELETE | `session` + `perm:agents.manage` | — | `…` | 200, 400 | — |

### PUT `/api/workbench/harnesses` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `value` | slug: the regex is declared BEFORE the max, so a bad long slug answers the pattern sentence. |
| `label` | `string(1, 60)` |  |
| `description` | `string?(300)` |  |
| `modelPrefix` | `string?(40)` |  |
| `invoke` | `string(1, 500)` |  |
| `jsonInvoke` | `string?(500)` |  |
| `guide` | `string(1, 2000)` |  |

## `/api/workbench/jobs`

Source: [`api/src/routes/workbench/workbench_jobs.rs`](../../api/src/routes/workbench/workbench_jobs.rs)

> /api/workbench/jobs. Workbench jobs from the human side. GET ?taskId= →
> the ticket's jobs (board members — this is how the plan-approval gate and
> PR links surface on the ticket). PUT → approve / reject an awaiting job
> (board editors; rejection abandons with the reason in the ticket's audit
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{jobs}` | 200, 400, 403, 404 | — |
| PUT | `session` | [body](#put-apiworkbenchjobs-body) | `{ok}` | 200, 400, 403, 404 | — |

### PUT `/api/workbench/jobs` body

| field | schema | notes |
| :--- | :--- | :--- |
| `jobId` | `uuid` |  |
| `action` | `enum(approve|reject|merge_testing)` |  |
| `note` | `string?(500)` |  |

## `/api/workbench/repo-requests`

Source: [`api/src/routes/workbench/workbench_repo_requests.rs`](../../api/src/routes/workbench/workbench_repo_requests.rs)

> /api/workbench/repo-requests. Agent repo-creation requests. GET → pending
> queue; PUT → approve (creates the repo via the App, auto-grants it to the
> requester) or reject. Admin — approval mints real org resources.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{requests}` | 200 | — |
| PUT | `admin` | [body](#put-apiworkbenchrepo-requests-body) | `{ok, repo, url}` | 200, 400, 404 | audit |

### PUT `/api/workbench/repo-requests` body

| field | schema | notes |
| :--- | :--- | :--- |
| `id` | `uuid` |  |
| `action` | `enum(approve|reject)` |  |

## `/api/workbench/repos/{agentId}`

Source: [`api/src/routes/workbench/workbench_repos_agent_id.rs`](../../api/src/routes/workbench/workbench_repos_agent_id.rs)

> /api/workbench/repos/{agentId}. Per-agent workbench repo grants —
> explicit, like MCP assignment. GET → the connection's reachable pool +
> this agent's grants; PUT → replace the grant set (validated against the
> pool). agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{available, granted}` | 200, 404 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchreposagentid-body) | `{granted}` | 200, 400, 404 | — |

### PUT `/api/workbench/repos/{agentId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `repos` | `string[](0, 200, 0, 100)` |  |


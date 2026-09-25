# API reference — workbench

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

6 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/workbench/env/{*repo}`](#apiworkbenchenvrepo) | GET | `session` + `perm:agents.manage` |
| [`/api/workbench/env/{*repo}`](#apiworkbenchenvrepo) | PATCH | `session` + `perm:agents.manage` |
| [`/api/workbench/flow`](#apiworkbenchflow) | GET | `session` + `perm:agents.manage` |
| [`/api/workbench/flow`](#apiworkbenchflow) | PUT | `session` + `perm:agents.manage` |
| [`/api/workbench/github`](#apiworkbenchgithub) | GET | `admin` |
| [`/api/workbench/github`](#apiworkbenchgithub) | PUT | `admin` |
| [`/api/workbench/github`](#apiworkbenchgithub) | DELETE | `admin` |
| [`/api/workbench/jobs`](#apiworkbenchjobs) | GET | `session` |
| [`/api/workbench/jobs`](#apiworkbenchjobs) | PUT | `session` |
| [`/api/workbench/repo-requests`](#apiworkbenchrepo-requests) | GET | `admin` |
| [`/api/workbench/repo-requests`](#apiworkbenchrepo-requests) | PUT | `admin` |
| [`/api/workbench/repos/{agentId}`](#apiworkbenchreposagentid) | GET | `session` + `perm:agents.manage` |
| [`/api/workbench/repos/{agentId}`](#apiworkbenchreposagentid) | PUT | `session` + `perm:agents.manage` |

## `/api/workbench/env/{*repo}`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_env_repo.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_env_repo.rs)

> /api/workbench/env/{repo}. The per-project env store's admin wire: PATCH
> to set/delete entries (values sealed server-side, never echoed), GET for
> the key list. The VALUES never leave the database through this route —
> the only reader is the fleet render, which materializes them into the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{repo, keys}` | 200, 400 | — |
| PATCH | `session` + `perm:agents.manage` | [body](#patch-apiworkbenchenvrepo-body) | `{repo, keys}` | 200, 400 | audit |

### PATCH `/api/workbench/env/{*repo}` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/workbench/flow`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_flow.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_flow.rs)

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

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_github.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_github.rs)

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

## `/api/workbench/jobs`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_jobs.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_jobs.rs)

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

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_repo_requests.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_repo_requests.rs)

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

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_repos_agent_id.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_repos_agent_id.rs)

> /api/workbench/repos/{agentId}. Per-agent workbench repo grants —
> explicit, like MCP assignment. GET → the connection's reachable pool +
> this agent's grants; PUT → replace the grant set (validated against the
> pool). agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{available, granted, rules, branches}` | 200, 404 | — |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkbenchreposagentid-body) | `{granted}` | 200, 400, 404 | — |

### PUT `/api/workbench/repos/{agentId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `repos` | `string[](0, 200, 0, 100)` |  |


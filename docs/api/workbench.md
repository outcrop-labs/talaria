# API reference — workbench

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

18 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/workbench/auth/v1/credential`](#apiworkbenchauthv1credential) | POST | `agent` |
| [`/api/workbench/auth/v1/credential/{id}/block`](#apiworkbenchauthv1credentialidblock) | POST | `agent` |
| [`/api/workbench/auth/v1/credential/{id}/block`](#apiworkbenchauthv1credentialidblock) | DELETE | `agent` |
| [`/api/workbench/auth/v1/credential/{id}/blocks`](#apiworkbenchauthv1credentialidblocks) | DELETE | `agent` |
| [`/api/workbench/auth/v1/credential/{id}/disable`](#apiworkbenchauthv1credentialiddisable) | POST | `agent` |
| [`/api/workbench/auth/v1/credential/{id}/refresh`](#apiworkbenchauthv1credentialidrefresh) | POST | `agent` |
| [`/api/workbench/auth/v1/healthz`](#apiworkbenchauthv1healthz) | GET | `public` |
| [`/api/workbench/auth/v1/snapshot`](#apiworkbenchauthv1snapshot) | GET | `agent` |
| [`/api/workbench/coding/accounts/{agentId}`](#apiworkbenchcodingaccountsagentid) | GET | `session` |
| [`/api/workbench/coding/accounts/{agentId}`](#apiworkbenchcodingaccountsagentid) | PUT | `session` |
| [`/api/workbench/coding/accounts/{agentId}`](#apiworkbenchcodingaccountsagentid) | DELETE | `session` |
| [`/api/workbench/coding/login`](#apiworkbenchcodinglogin) | POST | `session` |
| [`/api/workbench/coding/login/{id}`](#apiworkbenchcodingloginid) | GET | `session` |
| [`/api/workbench/coding/login/{id}`](#apiworkbenchcodingloginid) | PUT | `session` |
| [`/api/workbench/coding/login/{id}`](#apiworkbenchcodingloginid) | DELETE | `session` |
| [`/api/workbench/coding/pin/{taskId}`](#apiworkbenchcodingpintaskid) | GET | `session` |
| [`/api/workbench/coding/pin/{taskId}`](#apiworkbenchcodingpintaskid) | PUT | `session` |
| [`/api/workbench/coding/pin/{taskId}`](#apiworkbenchcodingpintaskid) | DELETE | `session` |
| [`/api/workbench/coding/services`](#apiworkbenchcodingservices) | GET | `session` |
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
| [`/api/workbench/repos/{agentId}`](#apiworkbenchreposagentid) | GET | `session` + `agent-reader` |
| [`/api/workbench/repos/{agentId}`](#apiworkbenchreposagentid) | PUT | `session` + `agent-manager` |

## `/api/workbench/auth/v1/credential`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential.rs)

> POST /api/workbench/auth/v1/credential — refused, deliberately.
>
> In omp's protocol this is how a credential gets INTO a broker: a person runs
> `omp login` on the broker host and the result is uploaded. Talaria's logins
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | — | `…` | 403 | — |

**POST** — Refused by design. Coding accounts are signed in by a person in Talaria's UI, never uploaded from a sandbox — an agent that could write its own credential could grant itself a subscription.

## `/api/workbench/auth/v1/credential/{id}/block`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_block.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_block.rs)

> The harness's rate-limit blocks, persisted through the broker.
>
>   POST   /api/workbench/auth/v1/credential/{id}/block   remember one
>   DELETE /api/workbench/auth/v1/credential/{id}/block   forget one
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiworkbenchauthv1credentialidblock-body) | `{ok}` | 200, 400 | — |
| DELETE | `agent` | [body](#delete-apiworkbenchauthv1credentialidblock-body) | `{ok}` | 200, 400 | — |

**POST** — Remember that a provider is rate-limiting one of this agent's coding accounts until a given time, so the next job does not rediscover it.

### POST `/api/workbench/auth/v1/credential/{id}/block` body

| field | schema | notes |
| :--- | :--- | :--- |
| `providerKey` | `string(1, 200)` |  |
| `blockScope` | `string?(200)` | An absent scope is the provider-wide block, which is the common case. |
| `blockedUntilMs` | `number? nullable(0)` |  |
| `updatedAtMs` | `number? nullable(0)` |  |

**DELETE** — Forget one remembered rate-limit block. An empty `blockScope` targets the provider-wide row.

### DELETE `/api/workbench/auth/v1/credential/{id}/block` body

| field | schema | notes |
| :--- | :--- | :--- |
| `providerKey` | `string(1, 200)` |  |
| `blockScope` | `string?(200)` |  |

## `/api/workbench/auth/v1/credential/{id}/blocks`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_blocks.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_blocks.rs)

> DELETE /api/workbench/auth/v1/credential/{id}/blocks. Forget every
> remembered rate-limit block for one of the calling agent's coding accounts
> — the client's "start clean" after a credential comes back to life.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| DELETE | `agent` | — | `{ok}` | 200 | — |

**DELETE** — Forget every remembered rate-limit block for this coding account.

## `/api/workbench/auth/v1/credential/{id}/disable`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_disable.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_disable.rs)

> POST /api/workbench/auth/v1/credential/{id}/disable. The harness telling us
> a credential is dead.
>
> omp calls this when a provider answers in a way that means "this grant will
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiworkbenchauthv1credentialiddisable-body) | `{ok}` | 200 | — |

**POST** — Mark one of the calling agent's coding-account credentials dead. The row and its cause are kept for the UI; snapshots stop serving it.

### POST `/api/workbench/auth/v1/credential/{id}/disable` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/workbench/auth/v1/credential/{id}/refresh`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_refresh.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_credential_id_refresh.rs)

> POST /api/workbench/auth/v1/credential/{id}/refresh. The callback that makes
> Talaria the canonical refresher.
>
> A snapshot hands the harness an access token and `__remote__` where the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | — | `{entry}` | 200, 409, 500, 502 | — |

**POST** — Refresh one of the calling agent's coding-account credentials through the provider's own refresher and re-seal it. The harness calls this because the snapshot it holds carries no refresh token.

## `/api/workbench/auth/v1/healthz`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_healthz.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_healthz.rs)

> /api/workbench/auth/v1/healthz. omp's auth-broker liveness probe — the one
> route in the protocol that carries no bearer, by the protocol's own
> definition. It says nothing about any agent, so there is nothing to leak.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{ok, version}` | 200 | — |

**GET** — Liveness for the agent-facing coding-account broker. Unauthenticated by protocol definition; reveals nothing beyond "this instance speaks it".

## `/api/workbench/auth/v1/snapshot`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_auth_snapshot.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_auth_snapshot.rs)

> /api/workbench/auth/v1/snapshot. The calling agent's coding accounts, as
> omp's auth-broker protocol spells them.
>
> Conditional-GET shaped: the client sends `If-None-Match: "<generation>"` and
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{generation, generatedAt, serverNowMs, refresher, credentials}` | 200 + varies | — |

**GET** — The calling agent's coding-account credentials for its omp harness: access tokens with `__remote__` in the refresh slot, so this instance stays the only thing that can refresh them. Scoped by the agent's own key — an agent can read nothing but its own accounts.

## `/api/workbench/coding/accounts/{agentId}`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_coding_accounts_agent_id.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_coding_accounts_agent_id.rs)

> /api/workbench/coding/accounts/{agentId}. One agent's coding accounts: what
> is signed in, which plan is its default, and which model fills each of the
> harness's roles.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{accountId, provider, email, source, gateway, models}` | 200, 404 | — |
| PUT | `session` | [body](#put-apiworkbenchcodingaccountsagentid-body) | `{accountId, roles}` | 200, 400, 404 | — |
| DELETE | `session` | [body](#delete-apiworkbenchcodingaccountsagentid-body) | `{ok}` | 200, 400, 404 | — |

**GET** — One agent's coding accounts with their per-plan role picks, plus what a job that pins nothing would run on right now.

**PUT** — Set the agent's default coding plan, or replace one plan's role picks. An `accountId` of null means the org's Talaria gateway in both cases.

### PUT `/api/workbench/coding/accounts/{agentId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `action` | `enum(default|roles)` |  |
| `accountId` | `number? nullable(1)` |  |

**DELETE** — Sign one coding account out. Its role picks and any ticket pins naming it go with it, and the credential is deleted from this instance.

### DELETE `/api/workbench/coding/accounts/{agentId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `accountId` | `number? nullable(1)` | Signing out names an account; there is no "sign out of the gateway". |

## `/api/workbench/coding/login`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_coding_login.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_coding_login.rs)

> /api/workbench/coding/login. Signing an agent in to a coding account.
>
> THE SHAPE OF A SIGN-IN. The omp auth bridge runs the provider's own flow and
> reports what it is waiting for; this route relays that to the UI and relays
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiworkbenchcodinglogin-body) | `{provider}` | 200, 400, 403, 404, 502 | — |

**POST** — Start a coding-account sign-in for one agent. Answers the login session to poll: a URL to open, a question to answer, or both.

### POST `/api/workbench/coding/login` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentId` | `uuid` |  |
| `provider` | `string(1, 100)` |  |

## `/api/workbench/coding/login/{id}`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_coding_login.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_coding_login.rs)

> /api/workbench/coding/login. Signing an agent in to a coding account.
>
> THE SHAPE OF A SIGN-IN. The omp auth bridge runs the provider's own flow and
> reports what it is waiting for; this route relays that to the UI and relays
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{phase, accountId, provider, email, orgName}` | 200, 403, 404, 500, 502 | — |
| PUT | `session` | [body](#put-apiworkbenchcodingloginid-body) | `…` | 200, 400, 404, 500, 502 | — |
| DELETE | `session` | — | `{ok}` | 200, 500 | — |

**GET** — Poll a sign-in. A finished flow is sealed into this instance in the same request that reads it — the bridge serves a credential once.

**PUT** — Answer the provider's pending question, or paste the authorization code when the browser could not reach this instance's loopback.

### PUT `/api/workbench/coding/login/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `value` | `string(1, 4096)` |  |

**DELETE** — Abandon a sign-in in flight.

## `/api/workbench/coding/pin/{taskId}`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_coding_pin_task_id.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_coding_pin_task_id.rs)

> /api/workbench/coding/pin/{taskId}. Which plan this ticket's coding work
> runs on.
>
> WHY A TICKET NEEDS ITS OWN ANSWER. A subscription runs out. Somebody is
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{role, model}` | 200 | — |
| PUT | `session` | [body](#put-apiworkbenchcodingpintaskid-body) | `{accountId, model}` | 200, 400, 404, 409 | — |
| DELETE | `session` | — | `…` | 200 | — |

**GET** — This ticket's pinned coding plan, the plans its agent could use, and what a job started now would resolve to.

**PUT** — Pin this ticket's coding work to one plan and optionally one model. `accountId: null` pins it to the org's Talaria gateway.

### PUT `/api/workbench/coding/pin/{taskId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `accountId` | `number? nullable(1)` | null is the gateway, not a validation error. |
| `model` | `string?(200)` |  |

**DELETE** — Unpin — this ticket goes back to the agent's default coding plan.

## `/api/workbench/coding/services`

Source: [`api/crates/talaria-routes-workbench/src/workbench/workbench_coding_services.rs`](../../api/crates/talaria-routes-workbench/src/workbench/workbench_coding_services.rs)

> /api/workbench/coding/services. The services a coding account can be signed
> in to, straight out of omp's own roster.
>
> The roster is NOT a list Talaria keeps: it comes from the omp auth bridge,
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{services, permitted}` | 200, 502 | — |

**GET** — The coding-account sign-in roster from omp's own auth rules, each entry with its flow shape and whether the org permits it. `?models=<provider>` instead answers that provider's model ids, for the role pickers.

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

> /api/workbench/flow. Per-repo git flow (the branch PRs target). GET →
> configured flows + the reachable pool; PUT → set one repo's flow.
> agents.manage.

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
| `action` | `enum(approve|reject)` |  |
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
| GET | `session` + `agent-reader` | — | `{available, granted, rules, branches}` | 200, 404 | — |
| PUT | `session` + `agent-manager` | [body](#put-apiworkbenchreposagentid-body) | `{granted}` | 200, 400, 404 | — |

### PUT `/api/workbench/repos/{agentId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `repos` | `string[](0, 200, 0, 100)` |  |


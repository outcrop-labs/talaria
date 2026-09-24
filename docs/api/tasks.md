# API reference — tasks

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

12 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/tasks/{id}`](#apitasksid) | GET | `dual` |
| [`/api/tasks/{id}`](#apitasksid) | PUT | `dual` |
| [`/api/tasks/{id}`](#apitasksid) | DELETE | `session` |
| [`/api/tasks/{id}/channel`](#apitasksidchannel) | POST | `session` |
| [`/api/tasks/{id}/comments`](#apitasksidcomments) | GET | `dual` |
| [`/api/tasks/{id}/comments`](#apitasksidcomments) | POST | `dual` |
| [`/api/tasks/{id}/dependencies`](#apitasksiddependencies) | POST | `dual` |
| [`/api/tasks/{id}/dependencies`](#apitasksiddependencies) | DELETE | `session` |
| [`/api/tasks/{id}/review`](#apitasksidreview) | POST | `session` |
| [`/api/tasks/{id}/usage`](#apitasksidusage) | GET | `dual` |
| [`/api/tasks/{id}/usage`](#apitasksidusage) | POST | `agent` |
| [`/api/tasks/{id}/watchers`](#apitasksidwatchers) | POST | `session` |
| [`/api/tasks/{id}/watchers`](#apitasksidwatchers) | DELETE | `session` |
| [`/api/tasks/{id}/work-session`](#apitasksidwork-session) | GET | `session` |
| [`/api/tasks/{id}/work-session/stop`](#apitasksidwork-sessionstop) | POST | `session` |
| [`/api/tasks/{id}/work-sessions`](#apitasksidwork-sessions) | GET | `session` |
| [`/api/workflows`](#apiworkflows) | GET | `session` |
| [`/api/workflows`](#apiworkflows) | POST | `session` + `perm:agents.manage` |
| [`/api/workflows/{id}`](#apiworkflowsid) | PUT | `session` + `perm:agents.manage` |
| [`/api/workflows/{id}`](#apiworkflowsid) | DELETE | `session` + `perm:agents.manage` |

## `/api/tasks/{id}`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id.rs)

> /api/tasks/{id}. One ticket.
> GET → full detail (task, comments, activity, watchers, reviews, judge,
> links, usage; + workflows for an agent caller). PUT → update; the
> human-in-the-loop guardrails live in update_task, not here — agents may
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 403, 404, 409 | — |
| PUT | `dual` | [body](#put-apitasksid-body) | `{task}` | 200, 400, 403, 404, 409 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404, 409 | — |

### PUT `/api/tasks/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string?(300)` |  |
| `description` | `nullish` |  |
| `status` | `string?(40)` |  |
| `priority` | `enum(low|medium|high|urgent)?` |  |
| `effort` | `nullish` |  |
| `assignees` | `string[]?(0, 200, 20)` |  |
| `dueDate` | `nullish` |  |
| `startDate` | `nullish` |  |
| `color` | `nullish` |  |
| `tags` | `string[]?(1, 40, 20)` | tags elements carry min(1) — `[""]` once minted a blank label on the board. `[]` stays legal: it clears the labels. |
| `outcome` | `nullish` |  |
| `resolution` | `nullish` |  |
| `errorMessage` | `nullish` |  |
| `archived` | `bool?` |  |
| `estimatedHours` | `nullish` |  |
| `parentId` | `nullish` |  |
| `addTimeSpentSeconds` | `number?(0, 86400, 30)` |  |
| `attachmentIds` | `uuid[]?(20)` | Full replacement list, same contract as chat messages: upload ids + knowledge/artifact refs. Omit both to leave attachments unchanged. |

## `/api/tasks/{id}/channel`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_channel.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_channel.rs)

> /api/tasks/{id}/channel.
>
> OPEN THE ROOM FOR A TICKET, creating it the first time.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{channelId}` | 200, 404, 409 | — |

## `/api/tasks/{id}/comments`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_comments.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_comments.rs)

> /api/tasks/{id}/comments. GET → the thread (board member or board-allowed
> agent). POST → add a comment; an agent goes through the one
> agent-authority predicate with intent Comment — board policy plus
> archival, and deliberately NOT the closed-status clause, because
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{comments}` | 200, 403, 404, 409 | — |
| POST | `dual` | [body](#post-apitasksidcomments-body) | `{comment}` | 200, 400, 403, 404, 409 | — |

### POST `/api/tasks/{id}/comments` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string(1, 20000)` |  |
| `parentId` | `uuid?` |  |

## `/api/tasks/{id}/dependencies`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_dependencies.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_dependencies.rs)

> /api/tasks/{id}/dependencies. POST { dependsOnId } → this ticket is
> blocked by another. DELETE → remove. Editors or board-allowed agents may
> add (part of triage); removal is human-only. The dependency target must
> live on the same board.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apitasksiddependencies-body) | `{ok}` | 200, 400, 403, 404, 409 | — |
| DELETE | `session` | [body](#delete-apitasksiddependencies-body) | `{ok}` | 200, 400, 403, 404, 409 | — |

### POST `/api/tasks/{id}/dependencies` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dependsOnId` | `string(1, 200)` |  |

### DELETE `/api/tasks/{id}/dependencies` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dependsOnId` | `string(1, 200)` |  |

## `/api/tasks/{id}/review`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_review.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_review.rs)

> /api/tasks/{id}/review. The human quality gate. Approve moves the ticket
> to the board's done column; reject sends it back to the board's first
> working column. Board owner/editor only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apitasksidreview-body) | `{task}` | 200, 400, 403, 404, 409 | — |

### POST `/api/tasks/{id}/review` body

| field | schema | notes |
| :--- | :--- | :--- |
| `status` | `enum(approved|rejected)` |  |
| `notes` | `string?(20000)` |  |

## `/api/tasks/{id}/usage`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_usage.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_usage.rs)

> /api/tasks/{id}/usage. Per-ticket token spend. POST (agents, via MCP
> log_usage): report tokens burned working this ticket — attributed to the
> agent's serving endpoint and priced like every other ledger row. GET: the
> rollup shown on the ticket.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 403, 404, 409 | — |
| POST | `agent` | [body](#post-apitasksidusage-body) | `{ok}` | 200, 400, 403, 404, 409 | — |

### POST `/api/tasks/{id}/usage` body

| field | schema | notes |
| :--- | :--- | :--- |
| `promptTokens` | `number(0, 100000)` |  |
| `completionTokens` | `number(0, 100000)` |  |
| `tier` | `nullish` | Model tier the work ran on (alias name); defaults to the agent's main. |
| `estimated` | `bool?` |  |

## `/api/tasks/{id}/watchers`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_watchers.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_watchers.rs)

> /api/tasks/{id}/watchers. POST { watcher } → follow.
> DELETE { watcher } → unfollow.
>
> WHO MAY DO WHAT, and why each is the role it is:
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apitasksidwatchers-body) | `{watchers}` | 200, 400, 403, 404, 409 | — |
| DELETE | `session` | [body](#delete-apitasksidwatchers-body) | `{watchers}` | 200, 400, 403, 404, 409 | — |

### POST `/api/tasks/{id}/watchers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `watcher` | `string(1, 200)` |  |

### DELETE `/api/tasks/{id}/watchers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `watcher` | `string(1, 200)` |  |

## `/api/tasks/{id}/work-session`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_work_session.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_work_session.rs)

> GET /api/tasks/{id}/work-session. The LIVE WORK SESSION on a ticket, for
> the detail view's ticker and its watch modal: the run row (state, phase,
> the agent working it, the turn count) plus the tail of the agent's last
> reply, read from the checkpoint the session persists before acting. This
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{session, wait}` | 200, 403, 404, 409 | — |

## `/api/tasks/{id}/work-session/stop`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_work_session_stop.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_work_session_stop.rs)

> POST /api/tasks/{id}/work-session/stop. THE HUMAN BRAKE. Once an agent
> has picked a ticket up, nothing else on the board ends that work: the
> session is a detached driver whose send step may legitimately run for
> hours. This route cancels every live work-session on the ticket —
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{ok}` | 200, 403, 404, 409 | — |

## `/api/tasks/{id}/work-sessions`

Source: [`api/crates/talaria-routes-boards/src/tasks/tasks_id_work_sessions.rs`](../../api/crates/talaria-routes-boards/src/tasks/tasks_id_work_sessions.rs)

> GET /api/tasks/{id}/work-sessions. THE RECORD, beside the live read's
> NOW. The live work-session read answers "is anyone on it"; this answers
> "what happened" — every session this ticket has had, live ones first by
> creation order, finished ones newest-first, twenty deep. The run-detail
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{sessions}` | 200, 403, 404, 409 | — |

## `/api/workflows`

Source: [`api/crates/talaria-routes-boards/src/tasks/workflows.rs`](../../api/crates/talaria-routes-boards/src/tasks/workflows.rs)

> /api/workflows. GET lists every workflow for any signed-in member (they
> ground what agents will be told — deliberately unscoped); POST is
> agents.manage. The body schema is shared with workflows_id:
> validate_workflow_body with post=false is the PUT patch — everything
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{workflows}` | 200 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apiworkflows-body) | `{workflow}` | 200, 400 | — |

### POST `/api/workflows` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(1, 80)` |  |
| `description` | `string?(500)` |  |
| `skills` | `string[]?(1, 80, 20)` |  |
| `enabled` | `bool` |  |

## `/api/workflows/{id}`

Source: [`api/crates/talaria-routes-boards/src/tasks/workflows_id.rs`](../../api/crates/talaria-routes-boards/src/tasks/workflows_id.rs)

> /api/workflows/{id}. PUT patch and DELETE, both agents.manage. Gate
> order: perm, body, then the SQL bind — so a member with a bad body gets
> the 403, and a non-uuid id only reaches the bind when a field is PRESENT
> (an empty patch runs no SQL and answers ok even for a non-uuid id).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkflowsid-body) | `{ok}` | 200, 400 | — |
| DELETE | `session` + `perm:agents.manage` | — | `{ok}` | 200 | — |

### PUT `/api/workflows/{id}` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.


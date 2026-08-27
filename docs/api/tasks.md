# API reference — tasks

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

8 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/tasks/{id}`](#apitasksid) | GET | `dual` |
| [`/api/tasks/{id}/comments`](#apitasksidcomments) | GET | `dual` |
| [`/api/tasks/{id}/comments`](#apitasksidcomments) | POST | `dual` |
| [`/api/tasks/{id}/dependencies`](#apitasksiddependencies) | POST | `dual` |
| [`/api/tasks/{id}/review`](#apitasksidreview) | POST | `session` |
| [`/api/tasks/{id}/usage`](#apitasksidusage) | GET | `dual` |
| [`/api/tasks/{id}/usage`](#apitasksidusage) | POST | `agent` |
| [`/api/tasks/{id}/watchers`](#apitasksidwatchers) | POST | `session` |
| [`/api/tasks/{id}/watchers`](#apitasksidwatchers) | DELETE | `session` |
| [`/api/workflows`](#apiworkflows) | GET | `session` |
| [`/api/workflows`](#apiworkflows) | POST | `session` + `perm:agents.manage` |
| [`/api/workflows/{id}`](#apiworkflowsid) | PUT | `session` + `perm:agents.manage` |
| [`/api/workflows/{id}`](#apiworkflowsid) | DELETE | `session` + `perm:agents.manage` |

## `/api/tasks/{id}`

Source: [`ui/src/routes/api/tasks.$id.ts`](../../ui/src/routes/api/tasks.$id.ts)

> One ticket. GET → full detail (task, comments, attachments, refs,
> workflows). PUT → update; agents may triage but cannot self-assign or
> move to done (coerced to quality_review). DELETE → owner/editor.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | [body](#get-apitasksid-body) | `{workflows}` | 200, 400, 403, 404 | — |

### GET `/api/tasks/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().min(1).max(300).optional()` |  |
| `description` | `z.string().max(20_000).nullish()` |  |

## `/api/tasks/{id}/comments`

Source: [`ui/src/routes/api/tasks.$id.comments.ts`](../../ui/src/routes/api/tasks.$id.comments.ts)

> GET → a task's comments (board member or board-allowed agent).
> POST → add a comment (member or agent).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{comments}` | 200, 404 | — |
| POST | `dual` | [body](#post-apitasksidcomments-body) | `…` | 200, 404 | — |

### POST `/api/tasks/{id}/comments` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `z.string().min(1).max(20_000)` |  |
| `parentId` | `Uuid.optional()` |  |

## `/api/tasks/{id}/dependencies`

Source: [`ui/src/routes/api/tasks.$id.dependencies.ts`](../../ui/src/routes/api/tasks.$id.dependencies.ts)

> POST { dependsOnId } → this ticket is blocked by another. DELETE → remove.
> Editors or board-allowed agents may add (part of triage); removal is human-only.
> The dependency target must live on the same board.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apitasksiddependencies-body) | `{ok}` | 200, 400, 403, 404 | — |

### POST `/api/tasks/{id}/dependencies` body

| field | schema | notes |
| :--- | :--- | :--- |
| `dependsOnId` | `Uuid` |  |

## `/api/tasks/{id}/review`

Source: [`ui/src/routes/api/tasks.$id.review.ts`](../../ui/src/routes/api/tasks.$id.review.ts)

> POST /api/tasks/:id/review — the human quality gate. Approve moves the task to
> the board's done column; reject sends it back to the board's first working
> column. Board owner/editor only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apitasksidreview-body) | `{task}` | 200, 400, 403, 404 | — |

### POST `/api/tasks/{id}/review` body

| field | schema | notes |
| :--- | :--- | :--- |
| `status` | `z.enum(['approved', 'rejected'])` |  |
| `notes` | `z.string().max(20_000).optional()` |  |

## `/api/tasks/{id}/usage`

Source: [`ui/src/routes/api/tasks.$id.usage.ts`](../../ui/src/routes/api/tasks.$id.usage.ts)

> Per-ticket token spend. POST (agents, via MCP log_usage): report tokens
> burned working this ticket — attributed to the agent's serving endpoint and
> priced like every other ledger row. GET: the rollup shown on the ticket.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 403, 404 | — |
| POST | `agent` | [body](#post-apitasksidusage-body) | `{ok}` | 200, 400, 403, 404 | — |

### POST `/api/tasks/{id}/usage` body

| field | schema | notes |
| :--- | :--- | :--- |
| `promptTokens` | `z.number().int().min(0).max(100_000_000)` |  |
| `completionTokens` | `z.number().int().min(0).max(100_000_000)` |  |

## `/api/tasks/{id}/watchers`

Source: [`ui/src/routes/api/tasks.$id.watchers.ts`](../../ui/src/routes/api/tasks.$id.watchers.ts)

> POST { watcher } → follow. DELETE { watcher } → unfollow.
>
> WHO MAY DO WHAT, and why each is the role it is:
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apitasksidwatchers-body) | `{watchers}` | 200, 400, 403, 404 | — |
| DELETE | `session` | [body](#delete-apitasksidwatchers-body) | `{unwatched}` | 200, 403, 404 | — |

### POST `/api/tasks/{id}/watchers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `watcher` | `z.string().min(1).max(200)` |  |

### DELETE `/api/tasks/{id}/watchers` body

| field | schema | notes |
| :--- | :--- | :--- |
| `watcher` | `z.string().min(1).max(200)` |  |

## `/api/workflows`

Source: [`ui/src/routes/api/workflows.ts`](../../ui/src/routes/api/workflows.ts)

> Task workflows — match rules classify tickets; the payload (bound Hermes
> skills + declared toolkits) rides with dispatched/picked-up work. GET → all (any
> member; they ground what agents will be told). POST → agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{workflows}` | 200 | — |
| POST | `session` + `perm:agents.manage` | [body](#post-apiworkflows-body) | `{workflow}` | 200 | — |

### POST `/api/workflows` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().trim().min(1).max(80)` |  |
| `description` | `z.string().max(500).optional()` |  |
| `match` | `Match.optional()` |  |
| `skills` | `z.array(z.string().min(1).max(80)).max(20).optional()` |  |
| `toolkits` | `z.array(Toolkit).max(20).optional()` |  |

## `/api/workflows/{id}`

Source: [`ui/src/routes/api/workflows.$id.ts`](../../ui/src/routes/api/workflows.$id.ts)

> One task workflow: PUT patch, DELETE remove — both agents.manage.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:agents.manage` | [body](#put-apiworkflowsid-body) | `{ok}` | 200 | — |
| DELETE | `session` + `perm:agents.manage` | — | `{ok}` | 200 | — |

### PUT `/api/workflows/{id}` body

Body schema `Body.partial()` is not an object literal in the route file — see the route source.


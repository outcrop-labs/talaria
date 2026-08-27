# API reference — boards

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

10 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/boards`](#apiboards) | GET | `dual` |
| [`/api/boards`](#apiboards) | POST | `session` + `perm:boards.create` |
| [`/api/boards/{id}`](#apiboardsid) | PATCH | `dual` |
| [`/api/boards/{id}`](#apiboardsid) | DELETE | `session` |
| [`/api/boards/{id}/agents`](#apiboardsidagents) | GET | `session` |
| [`/api/boards/{id}/agents`](#apiboardsidagents) | PUT | `dual` |
| [`/api/boards/{id}/events`](#apiboardsidevents) | GET | `session` |
| [`/api/boards/{id}/labels`](#apiboardsidlabels) | GET | `session` |
| [`/api/boards/{id}/labels`](#apiboardsidlabels) | POST | `session` |
| [`/api/boards/{id}/labels`](#apiboardsidlabels) | PUT | `session` |
| [`/api/boards/{id}/labels`](#apiboardsidlabels) | DELETE | `session` |
| [`/api/boards/{id}/members`](#apiboardsidmembers) | GET | `dual` |
| [`/api/boards/{id}/members`](#apiboardsidmembers) | POST | `dual` |
| [`/api/boards/{id}/members`](#apiboardsidmembers) | DELETE | `dual` |
| [`/api/boards/{id}/statuses`](#apiboardsidstatuses) | GET | `session` |
| [`/api/boards/{id}/statuses`](#apiboardsidstatuses) | POST | `session` |
| [`/api/boards/{id}/statuses`](#apiboardsidstatuses) | PUT | `session` |
| [`/api/boards/{id}/statuses`](#apiboardsidstatuses) | DELETE | `session` |
| [`/api/boards/{id}/tasks`](#apiboardsidtasks) | GET | `dual` |
| [`/api/boards/{id}/tasks`](#apiboardsidtasks) | POST | `dual` |
| [`/api/boards/{id}/templates`](#apiboardsidtemplates) | GET | `session` |
| [`/api/boards/{id}/templates`](#apiboardsidtemplates) | PUT | `session` |
| [`/api/boards/{id}/views`](#apiboardsidviews) | GET | `session` |
| [`/api/boards/{id}/views`](#apiboardsidviews) | POST | `session` |
| [`/api/boards/{id}/views`](#apiboardsidviews) | PUT | `session` |
| [`/api/boards/{id}/views`](#apiboardsidviews) | DELETE | `session` |

## `/api/boards`

Source: [`ui/src/routes/api/boards.ts`](../../ui/src/routes/api/boards.ts)

> GET /api/boards → boards the user owns or that are shared with them.
> Agent-key + x-agent-name → boards whose policy allows that agent; a personal
> assistant additionally sees its owner's boards (with the owner's role) so it
> can govern them on the owner's behalf.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{boards}` | 200 | — |
| POST | `session` + `perm:boards.create` | [body](#post-apiboards-body) | `{board}` | 200, 403 | — |

### POST `/api/boards` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(120)` |  |
| `teamId` | `Uuid.nullish()` |  |

## `/api/boards/{id}`

Source: [`ui/src/routes/api/boards.$id.ts`](../../ui/src/routes/api/boards.$id.ts)

> PATCH /api/boards/:id { name?, archived?, judgeMode? } → rename/archive/set the
> QA judge mode (owner/editor). DELETE → owner only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `dual` | [body](#patch-apiboardsid-body) | `{ok}` | 200, 400, 401, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PATCH `/api/boards/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(120).optional()` |  |
| `archived` | `z.boolean().optional()` |  |
| `judgeMode` | `z.enum(['inherit', 'off', 'advisory', 'enforcing']).optional()` |  |
| `teamId` | `Uuid.nullable().optional()` |  |
| `teamName` | `z.string().max(120).nullish()` |  |

## `/api/boards/{id}/agents`

Source: [`ui/src/routes/api/boards.$id.agents.ts`](../../ui/src/routes/api/boards.$id.agents.ts)

> GET → { allowAll, models }. PUT → set the board's agent policy (owner/editor,
> or a personal assistant acting as its owner): either the full { allowAll,
> models } shape, or incremental { add, remove } merged onto the current list
> (the assistant-friendly spelling). Boards are restrictive by default.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | — |
| PUT | `dual` | [body](#put-apiboardsidagents-body) | `…` | 200, 401, 403 | — |

### PUT `/api/boards/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `allowAll` | `z.boolean().optional()` |  |
| `models` | `z.array(z.string().max(200)).max(100).optional()` |  |
| `add` | `z.array(z.string().max(200)).max(100).optional()` |  |
| `remove` | `z.array(z.string().max(200)).max(100).optional()` |  |

## `/api/boards/{id}/events`

Source: [`ui/src/routes/api/boards.$id.events.ts`](../../ui/src/routes/api/boards.$id.events.ts)

> GET /api/boards/:id/events → SSE stream of this board's live events (task/
> comment changes). Auth-gated to board members. Powers multiplayer boards.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/boards/{id}/labels`

Source: [`ui/src/routes/api/boards.$id.labels.ts`](../../ui/src/routes/api/boards.$id.labels.ts)

> Board labels. GET → the registry (any member). POST create, PUT rename/
> recolor (rename cascades into tickets), DELETE (strips off tickets) —
> owner/editor.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{labels}` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidlabels-body) | `{label}` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apiboardsidlabels-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiboardsidlabels-body) | `{ok}` | 200, 403 | — |

### POST `/api/boards/{id}/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(40)` |  |
| `color` | `z.string().max(20).optional()` |  |

### PUT `/api/boards/{id}/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `labelId` | `Uuid` |  |
| `name` | `z.string().min(1).max(40).optional()` |  |
| `color` | `z.string().max(20).optional()` |  |

### DELETE `/api/boards/{id}/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `labelId` | `Uuid` |  |

## `/api/boards/{id}/members`

Source: [`ui/src/routes/api/boards.$id.members.ts`](../../ui/src/routes/api/boards.$id.members.ts)

> GET → members. POST { email, role } → share (owner/editor). DELETE { userId
> | email } → unshare. Write actions accept a personal assistant acting as its
> owner (identity proxy) alongside signed-in humans.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{members}` | 200, 403 | — |
| POST | `dual` | [body](#post-apiboardsidmembers-body) | `{ok}` | 200, 400, 401, 403 | audit |
| DELETE | `dual` | [body](#delete-apiboardsidmembers-body) | `{ok}` | 200, 400, 401, 403 | audit |

### POST `/api/boards/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `Email` |  |
| `role` | `z.enum(['editor', 'viewer']).default('editor')` |  |

### DELETE `/api/boards/{id}/members` body

Body schema `z.object({ userId: Uuid.optional(), email: Email.optional() }).refine((b) => b.userId || b.email, { message: 'userId or email required' })` is not an object literal in the route file — see the route source.

## `/api/boards/{id}/statuses`

Source: [`ui/src/routes/api/boards.$id.statuses.ts`](../../ui/src/routes/api/boards.$id.statuses.ts)

> A board's custom statuses and the diagnostics that explain whether
> agents may start/stop work on each. Writes are owner/editor.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidstatuses-body) | `{status}` | 200, 400, 403 + varies | — |
| PUT | `session` | [body](#put-apiboardsidstatuses-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiboardsidstatuses-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/boards/{id}/statuses` body

| field | schema | notes |
| :--- | :--- | :--- |
| `label` | `z.string().min(1).max(40)` |  |
| `color` | `z.string().max(20).optional()` |  |
| `category` | `Category.optional()` |  |
| `agentStart` | `z.boolean().optional()` |  |

### PUT `/api/boards/{id}/statuses` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `statusKey` | `z.string().min(1).max(40)` |  |
| `label` | `z.string().min(1).max(40).optional()` |  |
| `color` | `z.string().max(20).optional()` |  |
| `category` | `Category.optional()` |  |
| `agentStart` | `z.boolean().optional()` |  |

### PUT `/api/boards/{id}/statuses` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `order` | `z.array(z.string().min(1).max(40)).min(1).max(50)` |  |

### DELETE `/api/boards/{id}/statuses` body

| field | schema | notes |
| :--- | :--- | :--- |
| `statusKey` | `z.string().min(1).max(40)` |  |
| `reassignTo` | `z.string().max(40)` |  |

## `/api/boards/{id}/tasks`

Source: [`ui/src/routes/api/boards.$id.tasks.ts`](../../ui/src/routes/api/boards.$id.tasks.ts)

> GET → the board's tasks (any member, or a board-allowed agent).
> POST → create a card (owner/editor, or a board-allowed agent → inbox).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{tasks}` | 200 | — |
| POST | `dual` | [body](#post-apiboardsidtasks-body) | `…` | 200, 400, 403 | — |

### POST `/api/boards/{id}/tasks` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().min(1).max(300)` |  |
| `description` | `z.string().max(20_000).optional()` |  |
| `priority` | `z.enum(PRIORITIES).optional()` |  |
| `effort` | `z.enum(EFFORTS).nullish()` |  |
| `assignees` | `z.array(z.string().max(200)).max(20).optional()` |  |
| `dueDate` | `z.string().datetime().nullish()` |  |
| `startDate` | `z.string().datetime().nullish()` |  |
| `color` | `z.enum(TICKET_COLORS).nullish()` |  |
| `estimatedHours` | `z.number().min(0).max(999).nullish()` |  |
| `parentId` | `Uuid.nullish()` |  |
| `tags` | `z.array(z.string().max(40)).max(20).optional()` |  |

## `/api/boards/{id}/templates`

Source: [`ui/src/routes/api/boards.$id.templates.ts`](../../ui/src/routes/api/boards.$id.templates.ts)

> The ticket templates a board uses. GET → bindings. PUT { templateIds,
> defaultId } → replace the set (owner/editor); defaultId must be in the set.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{bindings}` | 200, 403 | — |
| PUT | `session` | [body](#put-apiboardsidtemplates-body) | `{bindings}` | 200, 400, 403 | — |

### PUT `/api/boards/{id}/templates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `templateIds` | `z.array(Uuid).max(50)` |  |
| `defaultId` | `Uuid.nullable()` |  |

## `/api/boards/{id}/views`

Source: [`ui/src/routes/api/boards.$id.views.ts`](../../ui/src/routes/api/boards.$id.views.ts)

> Saved board views — named filter/layout presets shared with the board.
> GET → list (any member); POST/PUT/DELETE → owner/editor. Config is the
> board URL's search state verbatim; the client owns its meaning.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidviews-body) | `{view}` | 200, 403 | — |
| PUT | `session` | [body](#put-apiboardsidviews-body) | `{ok}` | 200, 403 | — |
| DELETE | `session` | [body](#delete-apiboardsidviews-body) | `{ok}` | 200, 403 | — |

### POST `/api/boards/{id}/views` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(60)` |  |
| `config` | `Config` |  |

### PUT `/api/boards/{id}/views` body

| field | schema | notes |
| :--- | :--- | :--- |
| `viewId` | `Uuid` |  |
| `name` | `z.string().min(1).max(60).optional()` |  |
| `config` | `Config.optional()` |  |

### DELETE `/api/boards/{id}/views` body

| field | schema | notes |
| :--- | :--- | :--- |
| `viewId` | `Uuid` |  |


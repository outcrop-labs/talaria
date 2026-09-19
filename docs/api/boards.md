# API reference — boards

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

15 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/boards`](#apiboards) | GET | `dual` |
| [`/api/boards`](#apiboards) | POST | `session` + `perm:boards.create` |
| [`/api/boards/{id}`](#apiboardsid) | PATCH | `dual` |
| [`/api/boards/{id}`](#apiboardsid) | DELETE | `session` |
| [`/api/boards/{id}/agent-requests`](#apiboardsidagent-requests) | GET | `dual` |
| [`/api/boards/{id}/agent-requests`](#apiboardsidagent-requests) | POST | `dual` |
| [`/api/boards/{id}/agent-requests`](#apiboardsidagent-requests) | PUT | `dual` |
| [`/api/boards/{id}/agents`](#apiboardsidagents) | GET | `session` |
| [`/api/boards/{id}/agents`](#apiboardsidagents) | POST | `agent` |
| [`/api/boards/{id}/agents`](#apiboardsidagents) | PUT | `dual` |
| [`/api/boards/{id}/agents`](#apiboardsidagents) | DELETE | `agent` |
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
| [`/api/boards/{id}/work-sessions`](#apiboardsidwork-sessions) | GET | `session` |
| [`/api/boards/{id}/workchains`](#apiboardsidworkchains) | GET | `session` |
| [`/api/boards/{id}/workchains`](#apiboardsidworkchains) | POST | `session` |
| [`/api/workchains/{id}`](#apiworkchainsid) | PATCH | `session` |
| [`/api/workchains/{id}`](#apiworkchainsid) | DELETE | `session` |
| [`/api/workchains/{id}/steps`](#apiworkchainsidsteps) | POST | `session` |
| [`/api/workchains/{id}/steps`](#apiworkchainsidsteps) | DELETE | `session` |

## `/api/boards`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards.rs)

> /api/boards. GET → the boards the caller owns or that are shared with them;
> an agent key swaps the question
> for the boards whose POLICY allows that agent, plus — for a personal
> assistant — its owner's boards under the owner's role (the identity-proxy
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{boards}` | 200 | — |
| POST | `session` + `perm:boards.create` | [body](#post-apiboards-body) | `{board}` | 200, 400, 403 | — |

### POST `/api/boards` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 120)` |  |
| `teamId` | `uuid?` |  |

## `/api/boards/{id}`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id.rs)

> /api/boards/{id}. PATCH { name?, archived?, judgeMode?, teamId?, teamName? }
> → rename/archive/set the QA
> judge mode (owner/editor); a team move is owner-only because it changes who
> can see the board. DELETE → owner only. The identity here is ACTING user —
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `dual` | [body](#patch-apiboardsid-body) | `{ok}` | 200, 400, 401, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PATCH `/api/boards/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(120)` |  |
| `archived` | `bool?` |  |
| `judgeMode` | `enum(inherit|off|advisory|enforcing)?` |  |
| `teamId` | `uuid? nullable` |  |
| `teamName` | `string? nullable(120)` |  |

## `/api/boards/{id}/agent-requests`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_agent_requests.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_agent_requests.rs)

> /api/boards/{id}/agent-requests. The request half of board access for a
> personal assistant whose owner CANNOT read the board — self-service
> (`POST …/agents/self`) is deliberately not available to it, so this is the
> path around that hole.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{requests}` | 200, 401, 403 | — |
| POST | `dual` | [body](#post-apiboardsidagent-requests-body) | `…` | 200, 400, 403, 404 | audit |
| PUT | `dual` | [body](#put-apiboardsidagent-requests-body) | `{ok, agentModel, status}` | 200, 400, 401, 403, 404 | audit |

### POST `/api/boards/{id}/agent-requests` body

| field | schema | notes |
| :--- | :--- | :--- |
| `reason` | `string?(500)` |  |
| `agentModel` | `string?(200)` |  |
| `agentModel` | `string(1, 200)` |  |

### PUT `/api/boards/{id}/agent-requests` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentModel` | `string(1, 200)` |  |
| `action` | `enum(approve|reject)` |  |

## `/api/boards/{id}/agents`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_agents.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_agents.rs)

> /api/boards/{id}/agents. GET → { allowAll, models }. PUT → set the board's
> agent policy (owner/editor,
> or a personal assistant acting as its owner): either the full { allowAll,
> models } shape, or incremental { add, remove } merged onto the current list
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | — |
| POST | `agent` | — | `…` | 200, 403, 404 | audit |
| PUT | `dual` | [body](#put-apiboardsidagents-body) | `{allowAll, models}` | 200, 400, 401, 403 | audit |
| DELETE | `agent` | — | `…` | 200, 403, 404 | audit |

### PUT `/api/boards/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `allowAll` | `bool?` | PUT's fields in schema order: allowAll, models, add, remove — each an array of model ids, each id ≤200 chars, ≤100 of them. |
| `models` | `string[]?(0, 200, 100)` |  |
| `add` | `string[]?(0, 200, 100)` |  |
| `remove` | `string[]?(0, 200, 100)` |  |

## `/api/boards/{id}/events`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_events.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_events.rs)

> /api/boards/{id}/events. SSE stream of this board's live events
> (task/comment changes), auth-gated to board members. Powers multiplayer
> boards. The stream itself is realtime's (board:<id> topic, fed by the
> publish plane); this route is only the gate in front of it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/boards/{id}/labels`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_labels.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_labels.rs)

> /api/boards/{id}/labels. Board labels: GET → the registry (any member);
> POST create, PUT rename/recolor (a rename cascades into tickets), DELETE
> (strips off tickets) — owner/editor. The label helpers' refusal sentences
> ('label name required', 'no such label', 'unknown color') answer as 400s —
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{labels}` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidlabels-body) | `{label}` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apiboardsidlabels-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiboardsidlabels-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/boards/{id}/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 40)` |  |
| `color` | `string?(20)` | color is a free string on the wire (≤20); the palette decides what it means — anything off it coerces to slate inside create_label. |

### PUT `/api/boards/{id}/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `labelId` | `uuid` |  |
| `name` | `string?(40)` |  |
| `color` | `string?(20)` |  |

### DELETE `/api/boards/{id}/labels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `labelId` | `uuid` |  |

## `/api/boards/{id}/members`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_members.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_members.rs)

> /api/boards/{id}/members. GET → the member list. POST { email, role } →
> share; DELETE { userId |
> email } → unshare. Agents allowed on the board may READ membership (they
> would mutate it blind otherwise); the writes stay identity-proxied — a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{members}` | 200, 403 | — |
| POST | `dual` | [body](#post-apiboardsidmembers-body) | `{ok}` | 200, 400, 401, 403 | audit |
| DELETE | `dual` | [body](#delete-apiboardsidmembers-body) | `{ok}` | 200, 400, 401, 403 | audit |

### POST `/api/boards/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |
| `role` | `enum(editor|viewer)` |  |

### DELETE `/api/boards/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid?` | Both members optional; the 'userId or email required' refine runs AFTER the field checks. |
| `email` | `email` |  |

## `/api/boards/{id}/statuses`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_statuses.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_statuses.rs)

> /api/boards/{id}/statuses. Board statuses (custom workflow columns). GET →
> the ordered list incl. the
> system Blocked column, with the diagnostics that explain whether agents may
> start/stop work on each (any member — the reader who cannot fix it can at
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{statuses, diagnostics}` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidstatuses-body) | `{status}` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apiboardsidstatuses-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiboardsidstatuses-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/boards/{id}/statuses` body

| field | schema | notes |
| :--- | :--- | :--- |
| `label` | `string(1, 40)` |  |
| `color` | `string?(20)` |  |
| `category` | `enum(open|active|review|done)?` |  |
| `agentStart` | `bool?` |  |

### PUT `/api/boards/{id}/statuses` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `statusKey` | `string(1, 40)` |  |
| `label` | `string?(40)` |  |
| `color` | `string?(20)` |  |
| `category` | `enum(open|active|review|done)?` |  |
| `agentStart` | `bool?` |  |

### PUT `/api/boards/{id}/statuses` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `order` | `string[]?(1, 40, 50)` |  |

### DELETE `/api/boards/{id}/statuses` body

| field | schema | notes |
| :--- | :--- | :--- |
| `statusKey` | `string(1, 40)` |  |
| `reassignTo` | `string?(40)` |  |

## `/api/boards/{id}/tasks`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_tasks.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_tasks.rs)

> /api/boards/{id}/tasks. GET → the board's tasks (any member, or a
> board-allowed agent; only humans
> may ask for the archived tail). POST → create a card (owner/editor, or a
> board-allowed agent → inbox). The create's guardrails ride in the library
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{tasks}` | 200, 403 | — |
| POST | `dual` | [body](#post-apiboardsidtasks-body) | `{task}` | 200, 400, 403 | — |

### POST `/api/boards/{id}/tasks` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string(1, 300)` |  |
| `description` | `string?(20000)` |  |
| `priority` | `enum(low|medium|high|urgent)?` |  |
| `effort` | `enum(xs|s|m|l|xl)? nullish` |  |
| `assignees` | `string[]?(0, 200, 20)` |  |
| `dueDate` | `datetime? nullish` |  |
| `startDate` | `datetime? nullish` |  |
| `color` | `enum(slate|bronze|green|amber|red|blue|purple|teal|pink|orange|lime|cyan|indigo|magenta|olive|brown)? nullish` |  |
| `estimatedHours` | `number? nullable(0, 999)` |  |
| `parentId` | `uuid?` |  |
| `tags` | `string[]?(0, 40, 20)` |  |

## `/api/boards/{id}/templates`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_templates.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_templates.rs)

> /api/boards/{id}/templates. The ticket templates a board uses. GET → the
> bindings (any member); PUT
> {templateIds, defaultId} → replace the set (owner/editor). defaultId must
> be one of templateIds (null = no default); an empty templateIds clears the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{bindings}` | 200, 403 | — |
| PUT | `session` | [body](#put-apiboardsidtemplates-body) | `{bindings}` | 200, 400, 403 | — |

### PUT `/api/boards/{id}/templates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `templateIds` | `uuid[](50)` |  |
| `defaultId` | `uuid? nullable` |  |

## `/api/boards/{id}/views`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_views.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_views.rs)

> /api/boards/{id}/views. Saved board views: named filter/layout presets
> shared with the board.
> GET → the board's views (any member); POST → create; PUT → rename/update
> config; DELETE → remove (owner/editor). Config is the board URL's search
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{views}` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidviews-body) | `{view}` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apiboardsidviews-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiboardsidviews-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/boards/{id}/views` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 60)` |  |
| `view` | `enum(board|list|gantt|workchains)?` |  |

### PUT `/api/boards/{id}/views` body

| field | schema | notes |
| :--- | :--- | :--- |
| `viewId` | `uuid` |  |
| `name` | `string?(60)` |  |

### DELETE `/api/boards/{id}/views` body

| field | schema | notes |
| :--- | :--- | :--- |
| `viewId` | `uuid` |  |

## `/api/boards/{id}/work-sessions`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_work_sessions.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_work_sessions.rs)

> GET /api/boards/{id}/work-sessions. The board's LIVE WORK SESSIONS as a
> taskId → session map — the list view's question ("which cards are being
> worked right now?") answered in one read instead of one per card. Gated by
> board visibility exactly like the board read itself; the map carries only
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{sessions, waits}` | 200, 403 | — |

## `/api/boards/{id}/workchains`

Source: [`api/crates/talaria-api-routes/src/routes/boards/boards_id_workchains.rs`](../../api/crates/talaria-api-routes/src/routes/boards/boards_id_workchains.rs)

> /api/boards/{id}/workchains. The board's workchains — ordered pipelines
> of its tickets. GET → the chains with their steps joined to task
> summaries and the derived done/head/waiting state (any member, exactly
> the readers board configuration gets); POST { name } → create, positioned
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{workchains}` | 200, 403 | — |
| POST | `session` | [body](#post-apiboardsidworkchains-body) | `{workchain}` | 200, 400, 403 | — |

### POST `/api/boards/{id}/workchains` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 120)` |  |

## `/api/workchains/{id}`

Source: [`api/crates/talaria-api-routes/src/routes/workchains/workchains_id.rs`](../../api/crates/talaria-api-routes/src/routes/workchains/workchains_id.rs)

> /api/workchains/{id}. PATCH { name?, paused?, positions? } → rename,
> pause/unpause, reorder steps. DELETE → remove the chain (its tickets are
> untouched — the cascade fires the step rows, never the tasks; deleting a
> chain unlinks, it does not delete work).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apiworkchainsid-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PATCH `/api/workchains/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(120)` |  |
| `paused` | `bool?` |  |

## `/api/workchains/{id}/steps`

Source: [`api/crates/talaria-api-routes/src/routes/workchains/workchains_id.rs`](../../api/crates/talaria-api-routes/src/routes/workchains/workchains_id.rs)

> /api/workchains/{id}. PATCH { name?, paused?, positions? } → rename,
> pause/unpause, reorder steps. DELETE → remove the chain (its tickets are
> untouched — the cascade fires the step rows, never the tasks; deleting a
> chain unlinks, it does not delete work).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiworkchainsidsteps-body) | `{ok}` | 200, 400, 403, 409 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### POST `/api/workchains/{id}/steps` body

| field | schema | notes |
| :--- | :--- | :--- |
| `taskId` | `uuid` |  |
| `after` | `uuid?` |  |


# API reference — inbox

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

7 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/inbox/focus`](#apiinboxfocus) | GET | `session` |
| [`/api/inbox/focus/actions`](#apiinboxfocusactions) | POST | `session` |
| [`/api/inbox/focus/command`](#apiinboxfocuscommand) | POST | `session` |
| [`/api/inbox/focus/conversations`](#apiinboxfocusconversations) | GET | `session` |
| [`/api/inbox/focus/conversations`](#apiinboxfocusconversations) | POST | `session` |
| [`/api/inbox/focus/conversations/{id}`](#apiinboxfocusconversationsid) | GET | `session` |
| [`/api/inbox/focus/conversations/{id}`](#apiinboxfocusconversationsid) | DELETE | `session` |
| [`/api/inbox/focus/state`](#apiinboxfocusstate) | PUT | `session` |
| [`/api/inbox/focus/summary`](#apiinboxfocussummary) | GET | `session` |

## `/api/inbox/focus`

Source: [`api/src/routes/inbox/inbox_focus.rs`](../../api/src/routes/inbox/inbox_focus.rs)

> /api/inbox/focus. GET → the focus inbox queue: what the assistant has
> teed up for the caller. No options are taken (the queue defaults:
> enrich, no snoozed) — the query string is ignored entirely.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/inbox/focus/actions`

Source: [`api/src/routes/inbox/inbox_focus_actions.rs`](../../api/src/routes/inbox/inbox_focus_actions.rs)

> /api/inbox/focus/actions. POST → execute a focus-inbox action: fire an
> action, confirm or cancel a pending decision, undo the last one. Every
> button on a queue card funnels through this one entry, under the Inbox
> lock; the timeline entry is attached before the lock drops so the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiinboxfocusactions-body) | `{status, message}` | 200, 400, 409, 422 | — |

### POST `/api/inbox/focus/actions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `key` | `string?(600)` |  |
| `actionId` | `string?(100)` |  |
| `commandDecisionId` | `uuid?` |  |
| `decisionId` | `uuid?` |  |
| `confirmationToken` | `string(20, 200)` |  |
| `cancelDecisionId` | `uuid?` |  |
| `undoDecisionId` | `uuid?` |  |

## `/api/inbox/focus/command`

Source: [`api/src/routes/inbox/inbox_focus_command.rs`](../../api/src/routes/inbox/inbox_focus_command.rs)

> /api/inbox/focus/command. POST → run one instruction from the focus inbox
> panel through the assistant (normal / fast / plan mode, optional model
> overrides), as an SSE stream of named events — conversation, status,
> content, activity, done, error.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiinboxfocuscommand-body) | `…` | 200, 400, 409 | SSE |

### POST `/api/inbox/focus/command` body

| field | schema | notes |
| :--- | :--- | :--- |
| `key` | `string? nullish` |  |
| `surface` | `string? nullish(40)` |  |
| `instruction` | `string trimmed(1, 20000)` |  |
| `delegateModel` | `string? nullish(300)` |  |
| `responseModel` | `string? nullish(300)` |  |
| `mode` | `enum(normal|fast|plan)?` |  |
| `conversationId` | `uuid?` |  |
| `effort` | `string? nullish(24)` |  |
| `attachmentIds` | `uuid[]?(12)` |  |

## `/api/inbox/focus/conversations`

Source: [`api/src/routes/inbox/inbox_focus_conversations.rs`](../../api/src/routes/inbox/inbox_focus_conversations.rs)

> /api/inbox/focus/conversations. GET → the panel's chat picker. POST →
> start a fresh conversation instance. Segmentation is the context
> strategy: a new instance is how old context is shed, and it is the
> owner's choice to make (no budget imposes it).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{conversations}` | 200 | — |
| POST | `session` | — | `{conversation}` | 200, 201 | — |

## `/api/inbox/focus/conversations/{id}`

Source: [`api/src/routes/inbox/inbox_focus_conversations_id.rs`](../../api/src/routes/inbox/inbox_focus_conversations_id.rs)

> /api/inbox/focus/conversations/{id}. One conversation instance, by its
> path id. GET → its timeline page (cursor paginates); DELETE → archive it.
> Both are scoped inside the module to the caller's own inbox
> conversations, so an id from the picker can never touch one of their
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |
| DELETE | `session` | — | `{ok}` | 200, 404 | — |

## `/api/inbox/focus/state`

Source: [`api/src/routes/inbox/inbox_focus_state.rs`](../../api/src/routes/inbox/inbox_focus_state.rs)

> /api/inbox/focus/state. PUT → mark a focus item viewed, or snooze it
> until a time. Unlocked by design: the tables this writes
> (inbox_focus_state, the snooze's inbox_decisions row) are tables the
> assistant turn never touches, so a snooze mid-stream is a state change,
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` | [body](#put-apiinboxfocusstate-body) | `{ok}` | 200, 400, 409 | — |

### PUT `/api/inbox/focus/state` body

| field | schema | notes |
| :--- | :--- | :--- |
| `sourceType` | `enum(approval|task|channel|notification)` |  |
| `sourceId` | `string(1, 500)` |  |
| `snoozedUntil` | `datetime? nullable` |  |
| `viewed` | `bool?` |  |

## `/api/inbox/focus/summary`

Source: [`api/src/routes/inbox/inbox_focus_summary.rs`](../../api/src/routes/inbox/inbox_focus_summary.rs)

> /api/inbox/focus/summary. GET → the one-screen summary of the caller's
> focus state: how many items are queued (snoozed ones excluded), as
> {count}.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{count}` | 200 | — |


# API reference — inbox

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

Source: [`ui/src/routes/api/inbox.focus.ts`](../../ui/src/routes/api/inbox.focus.ts)

> The focus inbox queue — what the assistant has teed up for the caller.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/inbox/focus/actions`

Source: [`ui/src/routes/api/inbox.focus.actions.ts`](../../ui/src/routes/api/inbox.focus.actions.ts)

> Execute a focus-inbox action: fire an action, confirm or cancel a
> pending decision, undo the last one.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiinboxfocusactions-body) | `{status, message}` | 200, 409 + varies | — |

### POST `/api/inbox/focus/actions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `key` | `z.string().min(1).max(600).optional()` |  |
| `actionId` | `z.string().min(1).max(100).optional()` |  |
| `payload` | `z.unknown().optional()` |  |
| `commandDecisionId` | `Uuid.optional()` |  |
| `decisionId` | `Uuid.optional()` |  |
| `confirmationToken` | `z.string().min(20).max(200).optional()` |  |
| `cancelDecisionId` | `Uuid.optional()` |  |
| `undoDecisionId` | `Uuid.optional()` |  |

## `/api/inbox/focus/command`

Source: [`ui/src/routes/api/inbox.focus.command.ts`](../../ui/src/routes/api/inbox.focus.command.ts)

> Run one instruction from the focus inbox panel through the assistant
> (normal / fast / plan mode, optional model overrides).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiinboxfocuscommand-body) | `…` | 200, 409 | SSE |

### POST `/api/inbox/focus/command` body

| field | schema | notes |
| :--- | :--- | :--- |
| `key` | `z.string().min(1).max(600).nullable().optional()` |  |
| `instruction` | `z.string().trim().min(1).max(20_000)` |  |
| `delegateModel` | `z.string().max(300).nullable().optional()` |  |
| `responseModel` | `z.string().max(300).nullable().optional()` |  |
| `mode` | `z.enum(['normal', 'fast', 'plan']).default('normal')` |  |

## `/api/inbox/focus/conversations`

Source: [`ui/src/routes/api/inbox.focus.conversations.ts`](../../ui/src/routes/api/inbox.focus.conversations.ts)

> GET → the panel's chat picker. POST → start a fresh conversation instance.
> Segmentation is the context strategy: a new instance is how old context is
> shed, and it is the owner's choice to make (no budget imposes it).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{conversations}` | 200 | — |
| POST | `session` | — | `{conversation}` | 200, 201 | — |

## `/api/inbox/focus/conversations/{id}`

Source: [`ui/src/routes/api/inbox.focus.conversations.$id.ts`](../../ui/src/routes/api/inbox.focus.conversations.$id.ts)

> One conversation instance, by its path id. GET → its timeline page (cursor
> paginates); DELETE → archive it. Both are scoped inside the module to the
> caller's own inbox conversations, so an id from the picker can never touch
> one of their ordinary chats. Archiving, not deleting: the messages and the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 409 | — |
| DELETE | `session` | — | `{ok}` | 200, 404 | — |

## `/api/inbox/focus/state`

Source: [`ui/src/routes/api/inbox.focus.state.ts`](../../ui/src/routes/api/inbox.focus.state.ts)

> Mark a focus item viewed, or snooze it until a time.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` | [body](#put-apiinboxfocusstate-body) | `{ok}` | 200, 409 | — |

### PUT `/api/inbox/focus/state` body

Body schema `z.object({ sourceType: z.enum(FOCUS_SOURCE_TYPES), sourceId: z.string().min(1).max(500), snoozedUntil: z.string().datetime().nullable().opt…` is not an object literal in the route file — see the route source.

## `/api/inbox/focus/summary`

Source: [`ui/src/routes/api/inbox.focus.summary.ts`](../../ui/src/routes/api/inbox.focus.summary.ts)

> The one-screen summary of the caller's focus state — what's queued,
> what's in flight, what needs a decision.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |


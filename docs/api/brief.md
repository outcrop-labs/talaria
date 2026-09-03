# API reference — brief

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

5 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/brief`](#apibrief) | GET | `session` |
| [`/api/brief/delegate`](#apibriefdelegate) | GET | `session` |
| [`/api/brief/delegate`](#apibriefdelegate) | POST | `session` |
| [`/api/brief/item`](#apibriefitem) | POST | `session` |
| [`/api/brief/read`](#apibriefread) | POST | `session` |
| [`/api/brief/reply`](#apibriefreply) | POST | `session` |

## `/api/brief`

Source: [`api/src/routes/brief/brief.rs`](../../api/src/routes/brief/brief.rs)

> /api/brief. GET → the caller's daily brief: the assistant-assembled digest of what
> needs them. The read sweeps-if-due first, then answers with the document —
> or with WHICH kind of nothing, because the three absences render
> differently and collapsing them into one empty state is how a surface
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{absent, nextAt, agent}` | 200 | — |

## `/api/brief/delegate`

Source: [`api/src/routes/brief/brief_delegate.rs`](../../api/src/routes/brief/brief_delegate.rs)

> /api/brief/delegate. GET → the caller's live reply grants. POST
> { channelId, granted } → grant
> or revoke the assistant's reply-without-asking privilege, org-wide (null)
> or for one channel.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{grants}` | 200 | — |
| POST | `session` | [body](#post-apibriefdelegate-body) | `{grant, sent}` | 200, 400, 403 | — |

### POST `/api/brief/delegate` body

| field | schema | notes |
| :--- | :--- | :--- |
| `channelId` | `uuid? nullable` | channelId must be PRESENT; null is the standing, org-wide grant. |
| `granted` | `bool` |  |

## `/api/brief/item`

Source: [`api/src/routes/brief/brief_item.rs`](../../api/src/routes/brief/brief_item.rs)

> /api/brief/item. POST { sourceKey, action, tz } → check off, dismiss, or
> restore one brief
> line. The owner's own verdict on their own document — scoped to the
> caller's brief inside `mark_brief_item`, so a key belonging to somebody
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefitem-body) | `{ok}` | 200, 400, 404 | — |

### POST `/api/brief/item` body

| field | schema | notes |
| :--- | :--- | :--- |
| `sourceKey` | `string(1, 200)` |  |
| `action` | `enum(check|dismiss|restore)` |  |
| `tz` | `string? nullish` | the helper answers the already-flattened Option (absent and null are the same thing to the engine, which takes `tz: Option<&str>`). |

## `/api/brief/read`

Source: [`api/src/routes/brief/brief_read.rs`](../../api/src/routes/brief/brief_read.rs)

> /api/brief/read. POST { briefId, seq } → move the brief reader's cursor.
> The ONLY mutation
> this surface exposes — there is no edit, no dismiss and no delete, because
> the document is append-only and every one of those would be a rewrite
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefread-body) | `{ok}` | 200, 400 | — |

### POST `/api/brief/read` body

| field | schema | notes |
| :--- | :--- | :--- |
| `briefId` | `uuid` |  |
| `seq` | `number(0, 9007)` | seq: integer, min 0, no max below the safe-integer ceiling (2^53-1). read_seq is int8, so the whole safe range is storable — no int4 clamp. |

## `/api/brief/reply`

Source: [`api/src/routes/brief/brief_reply.rs`](../../api/src/routes/brief/brief_reply.rs)

> /api/brief/reply. POST { draftId, decision } → approve or reject a reply
> the assistant
> drafted: send it, or discard it.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefreply-body) | `{status}` | 200, 400, 404, 409 | — |

### POST `/api/brief/reply` body

| field | schema | notes |
| :--- | :--- | :--- |
| `draftId` | `uuid` |  |
| `decision` | `enum(approve|reject)` |  |


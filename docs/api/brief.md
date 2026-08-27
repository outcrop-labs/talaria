# API reference — brief

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

Source: [`ui/src/routes/api/brief.ts`](../../ui/src/routes/api/brief.ts)

> The caller's daily brief — the assistant-assembled digest of what needs
> them. GET sweeps-if-due, then returns the current brief.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/brief/delegate`

Source: [`ui/src/routes/api/brief.delegate.ts`](../../ui/src/routes/api/brief.delegate.ts)

> Grant or revoke the assistant's reply-without-asking privilege, org-wide
> (null) or for one channel. Owner-only by construction: not a Perm, and no
> route takes a user id — nobody can grant it on somebody else's behalf.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{grants}` | 200 | — |
| POST | `session` | [body](#post-apibriefdelegate-body) | `{revoked}` | 200, 403 | — |

### POST `/api/brief/delegate` body

| field | schema | notes |
| :--- | :--- | :--- |
| `channelId` | `Uuid.nullable()` |  |
| `granted` | `z.boolean()` |  |

## `/api/brief/item`

Source: [`ui/src/routes/api/brief.item.ts`](../../ui/src/routes/api/brief.item.ts)

> Check off, dismiss, or restore one brief item. The reader's timezone
> rides along so the change lands on the brief they are looking at.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefitem-body) | `{ok}` | 200, 404 | — |

### POST `/api/brief/item` body

| field | schema | notes |
| :--- | :--- | :--- |
| `sourceKey` | `z.string().min(1).max(200)` |  |
| `action` | `z.enum(['check', 'dismiss', 'restore'])` |  |
| `tz` | `z.string().max(64).nullable().optional()` |  |

## `/api/brief/read`

Source: [`ui/src/routes/api/brief.read.ts`](../../ui/src/routes/api/brief.read.ts)

> Move the brief reader's cursor — the only mutation this surface exposes.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefread-body) | `{ok}` | 200 | — |

### POST `/api/brief/read` body

| field | schema | notes |
| :--- | :--- | :--- |
| `briefId` | `Uuid` |  |
| `seq` | `z.number().int().min(0)` |  |

## `/api/brief/reply`

Source: [`ui/src/routes/api/brief.reply.ts`](../../ui/src/routes/api/brief.reply.ts)

> Approve or reject a reply the assistant drafted — send it, or discard it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefreply-body) | `{status}` | 200, 404, 409 + varies | — |

### POST `/api/brief/reply` body

| field | schema | notes |
| :--- | :--- | :--- |
| `draftId` | `Uuid` |  |
| `decision` | `z.enum(['approve', 'reject'])` |  |


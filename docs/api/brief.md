# API reference — brief

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
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

Source: [`api/src/routes/brief/brief.rs`](../../api/src/routes/brief/brief.rs)

> The caller's daily brief — the assistant-assembled digest of what needs
> them. GET sweeps-if-due, then returns the current brief.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/brief/delegate`

Source: [`api/src/routes/brief/brief_delegate.rs`](../../api/src/routes/brief/brief_delegate.rs)

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

Source: [`api/src/routes/brief/brief_item.rs`](../../api/src/routes/brief/brief_item.rs)

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

Source: [`api/src/routes/brief/brief_read.rs`](../../api/src/routes/brief/brief_read.rs)

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

Source: [`api/src/routes/brief/brief_reply.rs`](../../api/src/routes/brief/brief_reply.rs)

> Approve or reject a reply the assistant drafted — send it, or discard it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apibriefreply-body) | `{status}` | 200, 404, 409 + varies | — |

### POST `/api/brief/reply` body

| field | schema | notes |
| :--- | :--- | :--- |
| `draftId` | `Uuid` |  |
| `decision` | `z.enum(['approve', 'reject'])` |  |


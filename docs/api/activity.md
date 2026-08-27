# API reference — activity

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

6 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/activity`](#apiactivity) | GET | `session` |
| [`/api/alerts`](#apialerts) | GET | `session` |
| [`/api/cost`](#apicost) | GET | `session` + `view:/observability` |
| [`/api/history`](#apihistory) | GET | `session` |
| [`/api/home`](#apihome) | GET | `session` |
| [`/api/notifications`](#apinotifications) | GET | `session` |
| [`/api/notifications`](#apinotifications) | PUT | `session` |
| [`/api/notifications`](#apinotifications) | PATCH | `session` |

## `/api/activity`

Source: [`ui/src/routes/api/activity.ts`](../../ui/src/routes/api/activity.ts)

> The merged workspace activity feed, scoped to the requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{events}` | 200 | — |

## `/api/alerts`

Source: [`ui/src/routes/api/alerts.ts`](../../ui/src/routes/api/alerts.ts)

> Derived system alerts (no persistence) for the requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{alerts}` | 200 | — |

## `/api/cost`

Source: [`ui/src/routes/api/cost.ts`](../../ui/src/routes/api/cost.ts)

> GET /api/cost → the token ledger overview (totals, per-agent, per-day).
> Org-wide financials: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/history`

Source: [`ui/src/routes/api/history.ts`](../../ui/src/routes/api/history.ts)

> Version history for agent internals (one API over two stores), KB docs,
> artifacts, and agent-def versions. GET lists; ?id= returns one revision.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{content}` | 200, 400, 403, 404 | — |

## `/api/home`

Source: [`ui/src/routes/api/home.ts`](../../ui/src/routes/api/home.ts)

> The Home/Today summary for the signed-in user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/notifications`

Source: [`ui/src/routes/api/notifications.ts`](../../ui/src/routes/api/notifications.ts)

> The caller's notifications: list, unread count, mark-read, and their
> settings. Delivery-channel config is admin-gated.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{notifications, unread, delivery}` | 200 | — |
| PUT | `session` | [body](#put-apinotifications-body) | `{ok}` | 200 | — |
| PATCH | `session` | [body](#patch-apinotifications-body) | `{delivery, canSetDelivery}` | 200, 403 | audit |

### PUT `/api/notifications` body

| field | schema | notes |
| :--- | :--- | :--- |
| `ids` | `z.array(Uuid).max(200).optional()` |  |

### PATCH `/api/notifications` body

Body schema `z.object({ prefs: z.record(z.string().max(40), ROUTE).refine((p) => Object.keys(p).length > 0, { message: 'nothing to update' }).refine((p)…` is not an object literal in the route file — see the route source.


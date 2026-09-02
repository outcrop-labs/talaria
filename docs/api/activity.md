# API reference — activity

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
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

Source: [`api/src/routes/activity.rs`](../../api/src/routes/activity.rs)

> The merged workspace activity feed, scoped to the requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{events}` | 200 | — |

## `/api/alerts`

Source: [`api/src/routes/alerts.rs`](../../api/src/routes/alerts.rs)

> Derived system alerts (no persistence) for the requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{alerts}` | 200 | — |

## `/api/cost`

Source: [`api/src/routes/cost.rs`](../../api/src/routes/cost.rs)

> GET /api/cost → the token ledger overview (totals, per-agent, per-day).
> Org-wide financials: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/history`

Source: [`api/src/routes/history.rs`](../../api/src/routes/history.rs)

> Version history for agent internals (one API over two stores), KB docs,
> artifacts, and agent-def versions. GET lists; ?id= returns one revision.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{content}` | 200, 400, 403, 404 | — |

## `/api/home`

Source: [`api/src/routes/home.rs`](../../api/src/routes/home.rs)

> The Home/Today summary for the signed-in user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/notifications`

Source: [`api/src/routes/notifications.rs`](../../api/src/routes/notifications.rs)

> The caller's notifications: list, unread count, mark-read, and their
> settings. Delivery-channel config is admin-gated.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{notifications, unread, delivery, canSetDelivery}` | 200 | — |
| PUT | `session` | [body](#put-apinotifications-body) | `{ok}` | 200 | — |
| PATCH | `session` | [body](#patch-apinotifications-body) | `{delivery, canSetDelivery}` | 200, 403 | audit |

### PUT `/api/notifications` body

| field | schema | notes |
| :--- | :--- | :--- |
| `ids` | `z.array(Uuid).max(200).optional()` |  |

### PATCH `/api/notifications` body

Body schema `z.object({ prefs: z.record(z.string().max(40), ROUTE).refine((p) => Object.keys(p).length > 0, { message: 'nothing to update' }).refine((p)…` is not an object literal in the route file — see the route source.


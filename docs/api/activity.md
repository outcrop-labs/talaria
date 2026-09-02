# API reference — activity

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
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

Source: [`api/src/routes/activity/activity.rs`](../../api/src/routes/activity/activity.rs)

> GET /api/activity. The merged workspace activity feed, scoped to the
> requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/alerts`

Source: [`api/src/routes/activity/alerts.rs`](../../api/src/routes/activity/alerts.rs)

> /api/alerts. GET → derived system alerts (no persistence) for the
> requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{alerts}` | 200 | — |

## `/api/cost`

Source: [`api/src/routes/activity/cost.rs`](../../api/src/routes/activity/cost.rs)

> GET /api/cost. The token ledger overview (totals, per-agent, per-day).
> Org-wide financials: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/history`

Source: [`api/src/routes/activity/history.rs`](../../api/src/routes/activity/history.rs)

> /api/history. Version history for agent internals, one API over two
> stores:
>   snapshot store (internal_versions): skill, memory, kb-doc, kb-space, artifact
>   agent versions (agent_versions):    soul, config, personality
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 400, 403, 404 | — |

## `/api/home`

Source: [`api/src/routes/activity/home.rs`](../../api/src/routes/activity/home.rs)

> /api/home. GET → the Home/Today summary for the signed-in user.
>
> The digest job registers in main.rs's scheduler table, not here — this
> file is only the read.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/notifications`

Source: [`api/src/routes/activity/notifications.rs`](../../api/src/routes/activity/notifications.rs)

> /api/notifications. The caller's inbox: GET is the bell's one read (list,
> unread, prefs, digest, the instance switch, whether THIS user may flip it),
> PUT marks read, PATCH changes routing. The mail fan-out behind the prefs
> (the outbox, the drain) is scheduler plane, so it is not here; this file
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{notifications, unread, prefs, digest, delivery, canSetDelivery}` | 200 | — |
| PUT | `session` | [body](#put-apinotifications-body) | `{ok}` | 200, 400 | — |
| PATCH | `session` | [body](#patch-apinotifications-body) | `{prefs, digest, delivery, canSetDelivery}` | 200, 400, 403 | audit |

### PUT `/api/notifications` body

| field | schema | notes |
| :--- | :--- | :--- |
| `ids` | `uuid[]?(200)` | ids absent OR EMPTY both mean "all of mine" — the data layer folds them together. |

### PATCH `/api/notifications` body

| field | schema | notes |
| :--- | :--- | :--- |
| `digest` | `enum(on|off)` |  |


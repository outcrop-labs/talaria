# API reference — activity

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

19 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/activity`](#apiactivity) | GET | `session` |
| [`/api/alerts`](#apialerts) | GET | `session` |
| [`/api/cost`](#apicost) | GET | `session` + `view:/observability` |
| [`/api/cost/prices`](#apicostprices) | PUT | `admin` |
| [`/api/cost/prices`](#apicostprices) | DELETE | `admin` |
| [`/api/cost/unpriced`](#apicostunpriced) | GET | `admin` |
| [`/api/cost/unpriced/lookup`](#apicostunpricedlookup) | POST | `admin` |
| [`/api/history`](#apihistory) | GET | `session` |
| [`/api/home`](#apihome) | GET | `session` |
| [`/api/host`](#apihost) | GET | `session` + `view:/observability` |
| [`/api/notifications`](#apinotifications) | GET | `session` |
| [`/api/notifications`](#apinotifications) | PUT | `session` |
| [`/api/notifications`](#apinotifications) | PATCH | `session` |
| [`/api/notifications/ack`](#apinotificationsack) | POST | `session` |
| [`/api/notifications/test`](#apinotificationstest) | POST | `session` |
| [`/api/push/devices`](#apipushdevices) | GET | `session` |
| [`/api/push/devices/{id}`](#apipushdevicesid) | DELETE | `session` |
| [`/api/push/key`](#apipushkey) | GET | `session` |
| [`/api/push/subscribe`](#apipushsubscribe) | POST | `session` |
| [`/api/push/unsubscribe`](#apipushunsubscribe) | POST | `session` |
| [`/api/unreads`](#apiunreads) | GET | `session` |
| [`/api/working`](#apiworking) | GET | `session` |

## `/api/activity`

Source: [`api/crates/talaria-routes-comms/src/activity/activity.rs`](../../api/crates/talaria-routes-comms/src/activity/activity.rs)

> GET /api/activity. The merged workspace activity feed, scoped to the
> requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/alerts`

Source: [`api/crates/talaria-routes-comms/src/activity/alerts.rs`](../../api/crates/talaria-routes-comms/src/activity/alerts.rs)

> /api/alerts. GET → derived system alerts (no persistence) for the
> requesting user.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{alerts}` | 200 | — |

## `/api/cost`

Source: [`api/crates/talaria-routes-comms/src/activity/cost.rs`](../../api/crates/talaria-routes-comms/src/activity/cost.rs)

> GET /api/cost. The token ledger overview (totals, per-agent, per-day).
> Org-wide financials: admins + people granted the Observability view.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/cost/prices`

Source: [`api/crates/talaria-routes-comms/src/activity/cost_prices.rs`](../../api/crates/talaria-routes-comms/src/activity/cost_prices.rs)

> /api/cost/prices. An admin's pricing decision for one (endpoint, model)
> no published price covers. PUT `mode: "manual"` sets a rate per 1M
> input / output tokens (cache-write and cache-read optional; the input
> rate times 1.25 / 0.1 when absent); PUT `mode: "included"` marks it
> included in a plan or free — priced at $0 and labelled `included`, never
> a silent zero. DELETE removes the decision (404 when there is none).
> The endpoint is the ledger's own text, "" for usage booked without one,
> so a deleted endpoint's history can still be priced. Pricing is
> read-time: every past row of the model re-prices at once, and each
> answer says how (`repriced`). Precedence: the provider's own charge,
> then included, then a published price, then a manual rate.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apicostprices-body) | `{price, repriced}` | 200, 400 | audit |
| DELETE | `admin` | [body](#delete-apicostprices-body) | `{ok, repriced}` | 200, 400, 404 | audit |

### PUT `/api/cost/prices` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `enum(manual|included)` |  |
| `priceInPerMtok` | `number? nullable(0, 100000)` |  |
| `priceOutPerMtok` | `number? nullable(0, 100000)` |  |
| `priceCacheWritePerMtok` | `number? nullable(0, 100000)` |  |
| `priceCacheReadPerMtok` | `number? nullable(0, 100000)` |  |
| `note` | `string? nullish(200)` |  |
| `endpoint` | `string(0, 200)` |  |
| `model` | `string(1, 300)` |  |

### DELETE `/api/cost/prices` body

| field | schema | notes |
| :--- | :--- | :--- |
| `endpoint` | `string(0, 200)` |  |
| `model` | `string(1, 300)` |  |

## `/api/cost/unpriced`

Source: [`api/crates/talaria-routes-comms/src/activity/cost_unpriced.rs`](../../api/crates/talaria-routes-comms/src/activity/cost_unpriced.rs)

> GET /api/cost/unpriced. Every cloud model whose usage in the last 30
> days has no price — its endpoint, model, token counts and the reason
> (no published price found, the price source didn't answer, not looked
> up yet, the endpoint is gone) — and every admin pricing decision with
> what it prices now. Admins only: the fix is theirs to make.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{unpriced, prices}` | 200 | — |

## `/api/cost/unpriced/lookup`

Source: [`api/crates/talaria-routes-comms/src/activity/cost_unpriced_lookup.rs`](../../api/crates/talaria-routes-comms/src/activity/cost_unpriced_lookup.rs)

> POST /api/cost/unpriced/lookup. Retry the published-price lookup for one
> (endpoint, model) now, and wait for it: OpenRouter's public catalog,
> plus that model's per-endpoint prices on an OpenRouter endpoint. Answers
> whether the model is priced now and, when it is not, the reason the
> lookup recorded (the source's own words when it did not answer), with
> what the window's past rows price at (`repriced`). 404 when no cloud
> endpoint carries that name — a deleted endpoint's history is priced by
> hand instead (PUT /api/cost/prices).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | [body](#post-apicostunpricedlookup-body) | `{priced, reason, repriced}` | 200, 400, 404 | — |

### POST `/api/cost/unpriced/lookup` body

| field | schema | notes |
| :--- | :--- | :--- |
| `endpoint` | `string(1, 200)` |  |
| `model` | `string(1, 300)` |  |

## `/api/history`

Source: [`api/crates/talaria-routes-comms/src/activity/history.rs`](../../api/crates/talaria-routes-comms/src/activity/history.rs)

> /api/history. Version history for agent internals, one API over two
> stores:
>   snapshot store (internal_versions): skill, memory, profile, kb-doc,
>                                       kb-space, artifact
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 400, 403, 404 | — |

## `/api/home`

Source: [`api/crates/talaria-routes-comms/src/activity/home.rs`](../../api/crates/talaria-routes-comms/src/activity/home.rs)

> /api/home. GET → the Home/Today summary for the signed-in user.
>
> The digest job registers in main.rs's scheduler table, not here — this
> file is only the read.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/host`

Source: [`api/crates/talaria-routes-fleet/src/models/host.rs`](../../api/crates/talaria-routes-fleet/src/models/host.rs)

> The machine this api can see: CPU, load, memory, swap, disk per
> mount, and the processes using them. Admins and Observability
> grantees. A container without the host mounts says so instead of
> reporting its own cgroup.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `…` | 200 | — |

## `/api/notifications`

Source: [`api/crates/talaria-routes-comms/src/activity/notifications.rs`](../../api/crates/talaria-routes-comms/src/activity/notifications.rs)

> /api/notifications. The caller's inbox: GET is the bell's one read (list,
> unread, prefs, digest, the instance switch, whether THIS user may flip it,
> and the desktop-alerts soft ask's memory — and, since the contract, the v2
> rows, the For-you count, the notify seq read in the same statement as the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{notifications, unread, prefs, digest, delivery, canSetDelivery, prompt, unreadForYou, seq, classes, channels, dnd, emailFrequency, features, next, more}` | 200, 400 | — |
| PUT | `session` | [body](#put-apinotifications-body) | `{ok, read, archived, unread, unarchived}` | 200, 400 | — |
| PATCH | `session` | [body](#patch-apinotifications-body) | `{prefs, digest, delivery, canSetDelivery, prompt, channels, dnd, emailFrequency}` | 200, 400, 403 | audit |

**GET** — Optional query: `view=foryou|all|done`, `unread=1`, `category=`, `class=`, `kind=` (comma lists), `limit=1..100`, `before=<next>`; or `since=<seq>` (rows with a higher seq, in seq order, ≤ 200, with `more`). None is wave 1's newest 50. See docs/notifications/live-and-inbox.md.

**PUT** — A selector (`ids`, `href` or `entity`, in that precedence; none is every row) and at most one action (`archive`, `unarchive`, `unread`; none marks read). Publishes the read-sync event.

### PUT `/api/notifications` body

| field | schema | notes |
| :--- | :--- | :--- |
| `ids` | `uuid[]?(200)` | These rows. Absent or empty, with no other selector: every row of yours (wave 1's mark-all). |
| `href` | `string?(512)` | Every row of yours that points at this place. |
| `entity` | `entity` | {type, id}: every row of yours about that thing (never anyone else's). |
| `archive` | `literal(true)` | Optional. Done: archive, and mark read. |
| `unarchive` | `literal(true)` | Optional. Back from Done (stays read); needs a selector. |
| `unread` | `literal(true)` | Optional. Unread again (not Done rows); needs a selector. |

### PATCH `/api/notifications` body

| field | schema | notes |
| :--- | :--- | :--- |
| `digest` | `enum(on|off)` |  |
| `emailFrequency` | `enum(15m|1h|1d|off)` |  |

## `/api/notifications/ack`

Source: [`api/crates/talaria-routes-comms/src/activity/notifications_ack.rs`](../../api/crates/talaria-routes-comms/src/activity/notifications_ack.rs)

> POST /api/notifications/ack. A client SURFACED these rows to the person —
> toasted them to someone looking, or posted an OS banner (I-18) — and says
> so: `{ids: uuid[] ≤ 200}` → `{ok, acked}`. Only the caller's own rows are
> touched and the first ack wins (`surfaced_at`). W2-E's deferred push fires
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apinotificationsack-body) | `{ok, acked}` | 200, 400 | — |

### POST `/api/notifications/ack` body

| field | schema | notes |
| :--- | :--- | :--- |
| `ids` | `uuid[]()` |  |

## `/api/notifications/test`

Source: [`api/crates/talaria-routes-comms/src/activity/notifications_test.rs`](../../api/crates/talaria-routes-comms/src/activity/notifications_test.rs)

> POST /api/notifications/test. One test push to one of the caller's own
> devices, for the status panel: `{deviceId: uuid}` → `{ok, device: {id,
> status, ms, error}}`. 404 when the device is not the caller's; 429 with
> `Retry-After` and `{error, retryAfter}` past 5 calls in 10 minutes per
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apinotificationstest-body) | `{ok, device}` | 200, 400, 404, 429, 501 | — |

### POST `/api/notifications/test` body

| field | schema | notes |
| :--- | :--- | :--- |
| `deviceId` | `uuid` |  |

## `/api/push/devices`

Source: [`api/crates/talaria-routes-comms/src/activity/push.rs`](../../api/crates/talaria-routes-comms/src/activity/push.rs)

> /api/push — the browser's half of the closed-tab plane. GET /key hands the
> instance's VAPID public key (what pushManager.subscribe wants as
> applicationServerKey); POST /subscribe files a device browser's
> subscription (the push service's endpoint plus the two key halves RFC 8291
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{devices}` | 200 | — |

## `/api/push/devices/{id}`

Source: [`api/crates/talaria-routes-comms/src/activity/push.rs`](../../api/crates/talaria-routes-comms/src/activity/push.rs)

> /api/push — the browser's half of the closed-tab plane. GET /key hands the
> instance's VAPID public key (what pushManager.subscribe wants as
> applicationServerKey); POST /subscribe files a device browser's
> subscription (the push service's endpoint plus the two key halves RFC 8291
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| DELETE | `session` | — | `{ok}` | 200, 404 | — |

## `/api/push/key`

Source: [`api/crates/talaria-routes-comms/src/activity/push.rs`](../../api/crates/talaria-routes-comms/src/activity/push.rs)

> /api/push — the browser's half of the closed-tab plane. GET /key hands the
> instance's VAPID public key (what pushManager.subscribe wants as
> applicationServerKey); POST /subscribe files a device browser's
> subscription (the push service's endpoint plus the two key halves RFC 8291
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{publicKey}` | 200 | — |

## `/api/push/subscribe`

Source: [`api/crates/talaria-routes-comms/src/activity/push.rs`](../../api/crates/talaria-routes-comms/src/activity/push.rs)

> /api/push — the browser's half of the closed-tab plane. GET /key hands the
> instance's VAPID public key (what pushManager.subscribe wants as
> applicationServerKey); POST /subscribe files a device browser's
> subscription (the push service's endpoint plus the two key halves RFC 8291
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apipushsubscribe-body) | `{ok}` | 200, 400 | — |

### POST `/api/push/subscribe` body

| field | schema | notes |
| :--- | :--- | :--- |
| `endpoint` | `string(1)` |  |
| `keys` | `record` | keys: a record — with zod's own received-name per shape, so an array in the slot says "array", an absent one says "undefined". |

## `/api/push/unsubscribe`

Source: [`api/crates/talaria-routes-comms/src/activity/push.rs`](../../api/crates/talaria-routes-comms/src/activity/push.rs)

> /api/push — the browser's half of the closed-tab plane. GET /key hands the
> instance's VAPID public key (what pushManager.subscribe wants as
> applicationServerKey); POST /subscribe files a device browser's
> subscription (the push service's endpoint plus the two key halves RFC 8291
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apipushunsubscribe-body) | `{ok}` | 200, 400 | — |

### POST `/api/push/unsubscribe` body

| field | schema | notes |
| :--- | :--- | :--- |
| `endpoint` | `string(1)` |  |

## `/api/unreads`

Source: [`api/crates/talaria-routes-comms/src/activity/unreads.rs`](../../api/crates/talaria-routes-comms/src/activity/unreads.rs)

> /api/unreads. The rail badges' one read: how much is waiting in each part
> of the app, as { comms, plan, work, research, notifications }. Comms is
> rooms plus agent chats — both unread planes under one badge — and the
> counts are the SAME predicates the pills themselves ride (list_channels per
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{comms, plan, work, research, notifications}` | 200 | — |

## `/api/working`

Source: [`api/crates/talaria-routes-comms/src/activity/working.rs`](../../api/crates/talaria-routes-comms/src/activity/working.rs)

> /api/working. The tab bar's one read: which views an agent is working in
> right now, as { comms, work, boards }, each a Lead or null. A view the
> server has no predicate for has no key, so the client cannot light it.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |


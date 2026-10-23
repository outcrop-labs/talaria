# API reference — apps

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

3 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/app-host/{app}/rebuild`](#apiapp-hostapprebuild) | POST | `admin` |
| [`/api/apps`](#apiapps) | GET | `session` |
| [`/api/apps/{app}/*`](#apiappsapp) | ANY | `session` |

## `/api/app-host/{app}/rebuild`

Source: [`ui/src/routes/api/app-host.$app.rebuild.ts`](../../ui/src/routes/api/app-host.$app.rebuild.ts)

> POST /api/app-host/<slug>/rebuild — compile and load-probe one app now, and
> start its database (admin). Manage → Apps calls it before Enable and from
> Rebuild. It answers the build's outcome rather than failing the request: a
> broken app is an answer ({ ok: false, error }), not a server error.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `{ok, slug, build}` | 200, 400 | — |

## `/api/apps`

Source: [`api/crates/talaria-routes-fleet/src/apps/apps.rs`](../../api/crates/talaria-routes-fleet/src/apps/apps.rs)

> GET /api/apps. The signed-in view of installed apps: ENABLED apps only,
> manifest data the client needs to draw nav items, routes, and settings tabs.
> Per-user view gating happens client-side off the session's appGrants (and
> server-side at the app API gateway and the /app-builds/ door) — this list is
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/apps/{app}/*`

Source: [`ui/src/routes/api/apps.$app.$.ts`](../../ui/src/routes/api/apps.$app.$.ts)

> The app-server gateway: `/api/apps/<slug>/*` dispatches into the app's own `server.ts`. Every path under it is the app's; the host's own app routes are under `/api/app-host/`.
> The host authenticates, checks the app is enabled and the user may reach it, then hands over a context (user, grants, sub-path and segments, store, the platform API as the caller, the app's secrets, the abort signal). The contract: the SDK doc.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| ANY | `session` | — | `app-defined` | — | — |


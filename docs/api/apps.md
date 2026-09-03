# API reference — apps

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

2 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/apps`](#apiapps) | GET | `session` |
| [`/api/apps/{app}/*`](#apiappsapp) | ANY | `session` |

## `/api/apps`

Source: [`api/src/routes/apps/apps.rs`](../../api/src/routes/apps/apps.rs)

> GET /api/apps. The signed-in view of installed apps: ENABLED apps only,
> manifest data the client needs to draw nav items, routes, and settings tabs.
> Per-user view gating happens client-side off deniedViews (and server-side at
> the app API gateway) — this list is not secret, it is the platform's own menu.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/apps/{app}/*`

Source: [`ui/src/routes/api/apps.$app.$.ts`](../../ui/src/routes/api/apps.$app.$.ts)

> The app-server gateway: `/api/apps/<slug>/*` dispatches into the app's own `server.ts`.
> The host authenticates, checks the app is enabled and the user may reach it, then hands over a context (user, sub-path, namespaced store). The contract: the SDK doc.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| ANY | `session` | — | `app-defined` | — | — |


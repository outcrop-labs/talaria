# API reference — apps

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

6 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/app-host/{app}/harnesses/{id}/cases/{name}`](#apiapp-hostappharnessesidcasesname) | POST | `internal` |
| [`/api/app-host/{app}/lifecycle`](#apiapp-hostapplifecycle) | POST | `internal` |
| [`/api/app-host/{app}/rebuild`](#apiapp-hostapprebuild) | POST | `admin` |
| [`/api/app-host/harnesses`](#apiapp-hostharnesses) | GET | `internal` |
| [`/api/apps`](#apiapps) | GET | `session` |
| [`/api/apps/{app}/*`](#apiappsapp) | ANY | `session` |

## `/api/app-host/{app}/harnesses/{id}/cases/{name}`

Source: [`ui/src/routes/api/app-host.$app.harnesses.$id.cases.$name.ts`](../../ui/src/routes/api/app-host.$app.harnesses.$id.cases.$name.ts)

> POST /api/app-host/<slug>/harnesses/<id>/cases/<name> — replay one of an
> enabled app harness's eval fixtures with the model pinned to `model`,
> through the real runner with `harness_runs` and guard-findings filing
> off (the sweep must not write into what it is compared against). Answers
> the value, the model, the fixture's verdict (`pass`, `fail`, `gap`, or
> null when there was no value to grade) and the run's contract, repair,
> latency and token figures. Loopback only, with the internal credential:
> the api's fitness sweep is the one caller.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `…` | 200, 400, 404, 503 | — |

## `/api/app-host/{app}/lifecycle`

Source: [`ui/src/routes/api/app-host.$app.lifecycle.ts`](../../ui/src/routes/api/app-host.$app.lifecycle.ts)

> POST /api/app-host/<slug>/lifecycle — the host's steps of an app update and
> uninstall, called only by the api (talaria-apps `AppHost`) with its
> root-derived service credential; no session reaches it. Body: { step }.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `{ok}` | 200, 400 | — |

## `/api/app-host/{app}/rebuild`

Source: [`ui/src/routes/api/app-host.$app.rebuild.ts`](../../ui/src/routes/api/app-host.$app.rebuild.ts)

> POST /api/app-host/<slug>/rebuild — compile and load-probe one app now, and
> start its database (admin). Manage → Apps calls it before Enable and from
> Rebuild; the api calls it with its service credential right after an
> install placed the app, and after an update that had to swap back. It
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `admin` | — | `{ok, slug, build}` | 200, 400 | — |

## `/api/app-host/harnesses`

Source: [`ui/src/routes/api/app-host.harnesses.ts`](../../ui/src/routes/api/app-host.harnesses.ts)

> GET /api/app-host/harnesses — every enabled app's activity harnesses
> (`apps/<slug>/harnesses/*.ts`), as data: id, label, job, requirements,
> floor, guard rules, output kind, fixtures with their names and bands,
> and the source `app:<slug>`. Never code: the api registers them beside
> its builtins for the model-fitness matrix and replays their fixtures
> through `POST /api/app-host/<slug>/harnesses/<id>/cases/<name>`.
> `problems` names every harness file that was skipped and why. Loopback
> only, with the internal credential: the api is the one caller.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `internal` | — | `{harnesses, problems}` | 200, 503 | — |

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


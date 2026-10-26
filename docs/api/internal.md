# API reference — internal

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

2 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/internal/apps/{slug}/llm-key`](#apiinternalappsslugllm-key) | POST | `internal` |
| [`/api/internal/apps/mcp-sync`](#apiinternalappsmcp-sync) | POST | `internal` |

## `/api/internal/apps/{slug}/llm-key`

Source: [`api/crates/talaria-routes-admin/src/admin/internal_apps_llm_key.rs`](../../api/crates/talaria-routes-admin/src/admin/internal_apps_llm_key.rs)

> The app host asks the api for an enabled app's gateway key: one `tlk_`
> key named `app:<slug>`, minted on the first ask and sealed into
> `app_llm_keys`, then the same key on every later ask while it still opens
> and is live — a key that was revoked, replaced or can no longer be opened
> is rotated. The host makes every model call it runs on the app's behalf
> (a harness run from the app's server or tools, a fitness replay) on this
> key, through `POST /api/llm/v1/chat/completions`, and never hands it to
> app code. Disabling or uninstalling the app revokes it. Answers `{ key }`;
> 404 when the app is not enabled. Callable only with the root-derived
> service credential (`x-talaria-internal`); the host's proxy never forwards
> `/api/internal/*`.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `{key}` | 200, 404, 409 | — |

## `/api/internal/apps/mcp-sync`

Source: [`api/crates/talaria-routes-admin/src/admin/internal_apps_mcp_sync.rs`](../../api/crates/talaria-routes-admin/src/admin/internal_apps_mcp_sync.rs)

> The app host asks the api to reconcile the registry rows of app-published
> MCP servers with the apps: an enabled app with an mcp.ts gets a serving
> row, its catalog copied from the build's load probe, and the row of an app
> that is turned off, failed its build or lost its mcp.ts is turned off with
> the reason. A row that changes between serving and off rolls the agents
> that carry it, so a broken app's tools leave them and come back when it
> serves again; every agent, team and user grant on the row is kept. An
> admin's own enable/disable of the server is never changed. The host calls
> it after a probe writes a catalog and after a failed build turns an app
> off. The same pass revokes the gateway key of every app that is not
> enabled — the host turns a broken app off by writing the enabled set
> itself. Callable only with the root-derived service credential
> (`x-talaria-internal`); the host's proxy never forwards `/api/internal/*`.
> Answers the app server names that serve.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `{ok, servers}` | 200 | — |


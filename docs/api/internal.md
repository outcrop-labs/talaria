# API reference — internal

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

1 route.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/internal/apps/mcp-sync`](#apiinternalappsmcp-sync) | POST | `internal` |

## `/api/internal/apps/mcp-sync`

Source: [`api/crates/talaria-routes-admin/src/admin/internal_apps_mcp_sync.rs`](../../api/crates/talaria-routes-admin/src/admin/internal_apps_mcp_sync.rs)

> The app host asks the api to reconcile the registry rows of app-published
> MCP servers with the enabled apps: a row per enabled app with an mcp.ts,
> its catalog copied from the build's load probe, and the row of an app that
> is no longer enabled removed — rolling the agents that carried it. The host
> calls it after a probe writes a catalog and after a failed build turns an
> app off, so a broken app's tools leave the agents that had them. Callable
> only with the root-derived service credential (`x-talaria-internal`); the
> host's proxy never forwards `/api/internal/*`. Answers the app server names
> that remain.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `{ok, servers}` | 200 | — |


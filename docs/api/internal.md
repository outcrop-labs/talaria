# API reference — internal

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

4 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/internal/apps/{slug}/guard`](#apiinternalappsslugguard) | POST | `internal` |
| [`/api/internal/apps/{slug}/llm-key`](#apiinternalappsslugllm-key) | POST | `internal` |
| [`/api/internal/apps/agent-turn`](#apiinternalappsagent-turn) | POST | `internal` |
| [`/api/internal/apps/mcp-sync`](#apiinternalappsmcp-sync) | POST | `internal` |

## `/api/internal/apps/{slug}/guard`

Source: [`api/crates/talaria-routes-admin/src/admin/internal_apps_guard.rs`](../../api/crates/talaria-routes-admin/src/admin/internal_apps_guard.rs)

> The app host hands over the final reply of one harness run an app's
> code made, for the guard's semantic rules — the ones a pattern cannot
> answer and the operator's decision model judges (`implied_completion`:
> does the reply claim something was done that nothing in its tool record
> did?). `{ answer, tools, model, caller?, rules }`: `tools` is the run's
> tool record (the backing tools the host's own structural pass read),
> `rules` the semantic rules the harness's `guard.rules` keeps (all of
> them when it declares none) and an admin has on — the host asks
> nothing when that leaves none. The host runs every structural rule
> itself and the relay runs none for an app's key, so each rule is judged
> once: the structural ones there, these here. A rule is judged only
> while an admin has it on and a decision model is configured; the pass
> runs detached, after the reply has gone back to the app, so it records
> findings under the run's caller stamp (kept inside `app:<slug>`) and
> can never change the answer. Answers 202 with the rules it took; 404
> when the app is not enabled. Callable only with the root-derived
> service credential (`x-talaria-internal`); the host's proxy never
> forwards `/api/internal/*`.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `{ok, rules}` | 200, 202, 400, 404 | — |

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

## `/api/internal/apps/agent-turn`

Source: [`api/crates/talaria-routes-admin/src/admin/internal_apps_agent_turn.rs`](../../api/crates/talaria-routes-admin/src/admin/internal_apps_agent_turn.rs)

> The app host sends one agent turn an app's code asked for
> (`ctx.agents.turn` in `@talaria/sdk/server`): `{ app, actor, agent,
> message, system?, session?, effort? }`, where `actor` is what started
> the app's code as the host authenticated it — `{ kind: "request",
> userId }`, `{ kind: "mcp", agent, proven }` or `{ kind: "trigger",
> trigger }`. Who may drive whom is decided here: from a request, only an
> agent that person may use (never someone else's personal assistant);
> from an MCP tool, only the calling agent, proven by its own key; from a
> trigger, no agent. A refusal is a 403 with the sentence, before any turn
> starts. The app must be enabled with a ready build and under its LLM
> budget (`app:<slug>`, the subject its relay calls are held to). The turn goes
> through the fleet's own chat door — on a Hermes session keyed per app,
> per person or calling agent, per `session` key, when one is given — and
> its spend is booked to the agent under ledger source `app`. The answer
> streams as newline-delimited JSON: `{type:"text",text}` and
> `{type:"tool",name,status,label}` as they land, then one
> `{type:"done",text,tools,session}` or `{type:"error",error}`. A turn
> silent for ten minutes is abandoned; a caller that hangs up stops the
> agent's run. Callable only with the root-derived service credential
> (`x-talaria-internal`); the host's proxy never forwards `/api/internal/*`.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `internal` | — | `…` | 200, 400, 403 | SSE |

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


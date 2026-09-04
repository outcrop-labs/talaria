# API reference — llm

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.
> 
> **This group is the OpenAI-compatible wire** (`llm.v1.*`): external
> clients speak OpenAI shapes here, NOT house conventions — see the source
> before building against it.

2 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/llm/v1/chat/completions`](#apillmv1chatcompletions) | POST | `bearer-key` |
| [`/api/llm/v1/models`](#apillmv1models) | GET | `bearer-key` |

## `/api/llm/v1/chat/completions`

Source: [`api/src/routes/llm/llm_chat.rs`](../../api/src/routes/llm/llm_chat.rs)

> POST /api/llm/v1/chat/completions. OpenAI-compatible chat over the org's
> model stack: streaming and non-streaming both relay, every call metered
> into the ledger under the calling key's identity, every completion through
> the confab guard (gateway/guard.rs).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `bearer-key` | — | `…` | 200, 400, 401, 404, 429, 502 | SSE |

## `/api/llm/v1/models`

Source: [`api/src/routes/llm/llm_models.rs`](../../api/src/routes/llm/llm_models.rs)

> GET /api/llm/v1/models. OpenAI-compatible model list for external tools
> pointing a tlk_ key at base_url http://<talaria>/api/llm/v1. Byte-stability
> matters: clients diff this list, so field order and the 401 envelope are
> pinned (error.rs tests).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `bearer-key` | — | `…` | 200, 401 | — |


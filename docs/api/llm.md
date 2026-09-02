# API reference — llm

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

Source: [`api/src/routes/llm_chat.rs`](../../api/src/routes/llm_chat.rs)

> OpenAI-compatible chat completions — the wire external tools speak.
> Auth is a personal gateway API key (Bearer), not a session.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `bearer-key` | — | `…` | 200, 400, 401, 404, 429, 502 + varies | SSE |

## `/api/llm/v1/models`

Source: [`api/src/routes/llm_models.rs`](../../api/src/routes/llm_models.rs)

> OpenAI-compatible model list for the Talaria LLM gateway. External tools
> point at base_url http://<talaria>/api/llm/v1 with a minted tlk_ key.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `bearer-key` | — | `{object, data}` | 200, 401 | — |


# API reference — llm

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
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

Source: [`ui/src/routes/api/llm.v1.chat.completions.ts`](../../ui/src/routes/api/llm.v1.chat.completions.ts)

> OpenAI-compatible chat completions — the wire external tools speak.
> Auth is a personal gateway API key (Bearer), not a session.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `bearer-key` | — | `…` | 200, 400, 401, 404, 502 + varies | SSE |

## `/api/llm/v1/models`

Source: [`ui/src/routes/api/llm.v1.models.ts`](../../ui/src/routes/api/llm.v1.models.ts)

> OpenAI-compatible model list for the Talaria LLM gateway. External tools
> point at base_url http://<talaria>/api/llm/v1 with a minted tlk_ key.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `bearer-key` | — | `{object, data}` | 200, 401 | — |


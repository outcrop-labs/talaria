# API reference — models

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

5 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/inference`](#apiinference) | GET | `session` + `view:/observability` |
| [`/api/keys`](#apikeys) | GET | `session` |
| [`/api/keys`](#apikeys) | POST | `session` |
| [`/api/keys/{id}`](#apikeysid) | DELETE | `session` |
| [`/api/keys/{id}`](#apikeysid) | PUT | `session` |
| [`/api/models`](#apimodels) | GET | `session` |
| [`/api/models/efforts`](#apimodelsefforts) | GET | `session` |

## `/api/inference`

Source: [`api/src/routes/models/inference.rs`](../../api/src/routes/models/inference.rs)

> Local inference: your own hardware's backends (class=local), probed live,
> plus what they've served from the token ledger. Config lives on /models.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `{live, usage}` | 200 | — |

## `/api/keys`

Source: [`api/src/routes/models/keys.rs`](../../api/src/routes/models/keys.rs)

> Personal API keys for the Talaria LLM gateway. GET → my keys (+ whether I
> may mint). POST → mint one; the secret is in THIS response only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{keys, canMint}` | 200 | — |
| POST | `session` | [body](#post-apikeys-body) | `…` | 200, 403 | audit |

### POST `/api/keys` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(60)` |  |

## `/api/keys/{id}`

Source: [`api/src/routes/models/keys_id.rs`](../../api/src/routes/models/keys_id.rs)

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| DELETE | `session` | — | `{ok}` | 200 | audit |
| PUT | `session` | [body](#put-apikeysid-body) | `{ok}` | 200, 404 | audit |

### PUT `/api/keys/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `spendCapTokens` | `z.number().int().min(0).max(1e15).nullish()` |  |
| `spendCapUsd` | `z.number().min(0).max(1e9).nullish()` |  |
| `rateLimitPerMinute` | `z.number().int().min(0).max(10_000).nullish()` |  |

## `/api/models`

Source: [`api/src/routes/models/models.rs`](../../api/src/routes/models/models.rs)

> The gateway model catalog for signed-in users (the /api/llm/v1/models twin
> without an API key) — powers the preferred-model picker. Role-filtered:
> members see only what the admin allowlist permits; admins see everything.
> Each model carries a pretty label + a "what it's good at" blurb when the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{effective}` | 200 | — |

## `/api/models/efforts`

Source: [`api/src/routes/models/models_efforts.rs`](../../api/src/routes/models/models_efforts.rs)

> The composer's effort-picker feed: which reasoning-effort levels THIS model
> id may be asked for, plus the default it should start from. Thin by the house
> rule (routes parse and serialize; the decisions live in
> `server/model-efforts.ts` and `server/harness/persona.ts`) — the route adds
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{efforts, default}` | 200, 400 | — |


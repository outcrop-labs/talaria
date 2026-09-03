# API reference — models

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

5 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/inference`](#apiinference) | GET | `session` + `view:/observability` |
| [`/api/keys`](#apikeys) | GET | `session` |
| [`/api/keys`](#apikeys) | POST | `session` |
| [`/api/keys/{id}`](#apikeysid) | PUT | `session` |
| [`/api/keys/{id}`](#apikeysid) | DELETE | `session` |
| [`/api/models`](#apimodels) | GET | `session` |
| [`/api/models/efforts`](#apimodelsefforts) | GET | `session` |

## `/api/inference`

Source: [`api/src/routes/models/inference.rs`](../../api/src/routes/models/inference.rs)

> /api/inference.
> Local inference: your own hardware's backends (class=local), probed live,
> plus what they've served from the token ledger. Config lives on /models.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `view:/observability` | — | `{llmModel, tokens}` | 200 | — |

## `/api/keys`

Source: [`api/src/routes/models/keys.rs`](../../api/src/routes/models/keys.rs)

> /api/keys. Personal API keys for the Talaria LLM gateway. GET → my keys +
> whether I may mint. POST → mint one; the secret is in THIS response only
> (never stored, never logged, never in the audit entry).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{keys, canMint}` | 200 | — |
| POST | `session` | [body](#post-apikeys-body) | `{key, secret}` | 200, 400, 403 | audit |

### POST `/api/keys` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 60)` |  |

## `/api/keys/{id}`

Source: [`api/src/routes/models/keys_id.rs`](../../api/src/routes/models/keys_id.rs)

> /api/keys/{id}. DELETE revokes one of MY keys (the hash stays for audit);
> PUT sets my key's self-imposed policy (#265). A non-uuid {id} logs and
> answers the house envelope 500 — not a thrown platform error.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` | [body](#put-apikeysid-body) | `{ok, policy}` | 200, 400, 404 | audit |
| DELETE | `session` | — | `{ok}` | 200 | audit |

### PUT `/api/keys/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `spendCapTokens` | `number? nullable(0, 1)` |  |
| `spendCapUsd` | `number? nullable(0, 1)` |  |
| `rateLimitPerMinute` | `number? nullable(0, 10000)` |  |

## `/api/models`

Source: [`api/src/routes/models/models.rs`](../../api/src/routes/models/models.rs)

> /api/models.
> The gateway model catalog for signed-in users (the /api/llm/v1/models
> twin without an API key) — powers the preferred-model picker.
> Role-filtered: members see only what the admin allowlist permits; admins
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{models, effective}` | 200 | — |

## `/api/models/efforts`

Source: [`api/src/routes/models/models_efforts.rs`](../../api/src/routes/models/models_efforts.rs)

> /api/models/efforts.
> The composer's effort-picker feed: which reasoning-effort levels THIS
> model id may be asked for, plus the default it should start from. Thin by
> the house rule (routes parse and serialize; the decisions live in
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{efforts, default}` | 200, 400 | — |


# API reference — plans

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

3 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/plans/{id}/doc`](#apiplansiddoc) | GET | `session` |
| [`/api/plans/{id}/doc`](#apiplansiddoc) | POST | `session` |
| [`/api/plans/{id}/draft`](#apiplansiddraft) | GET | `session` |
| [`/api/plans/{id}/draft`](#apiplansiddraft) | POST | `session` |
| [`/api/plans/{id}/draft`](#apiplansiddraft) | PATCH | `session` |
| [`/api/plans/{id}/draft`](#apiplansiddraft) | DELETE | `session` |
| [`/api/plans/{id}/members`](#apiplansidmembers) | GET | `session` |
| [`/api/plans/{id}/members`](#apiplansidmembers) | POST | `session` |
| [`/api/plans/{id}/members`](#apiplansidmembers) | PUT | `session` |
| [`/api/plans/{id}/members`](#apiplansidmembers) | DELETE | `session` |

## `/api/plans/{id}/doc`

Source: [`api/src/routes/plans/plans_id_doc.rs`](../../api/src/routes/plans/plans_id_doc.rs)

> /api/plans/{id}/doc.
> The plan's living document (a linked doc artifact). GET → find-or-create
> it, seeded from the agent's plan template when one is bound. POST → the
> plan's agent rewrites it from the conversation so far. Owner or plan
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{artifact}` | 200, 404 | — |
| POST | `session` | [body](#post-apiplansiddoc-body) | `{artifactId}` | 200, 400, 403, 404, 502 | — |

### POST `/api/plans/{id}/doc` body

| field | schema | notes |
| :--- | :--- | :--- |
| `tier` | `string? nullish(60)` |  |

## `/api/plans/{id}/draft`

Source: [`api/src/routes/plans/plans_id_draft.rs`](../../api/src/routes/plans/plans_id_draft.rs)

> /api/plans/{id}/draft.
> The plan surface's ticket drafts as a DURABLE JOB: POST enqueues a
> 'plan-draft' run on the CONVERSATION'S OWN agent (the body names no agent
> — that is the one difference from the channel twin) and answers with the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{draft}` | 200, 404 | — |
| POST | `session` | [body](#post-apiplansiddraft-body) | `{draft}` | 200, 400, 403, 404, 500 | — |
| PATCH | `session` | [body](#patch-apiplansiddraft-body) | `{ok}` | 200, 400, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 404 | — |

### POST `/api/plans/{id}/draft` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

### PATCH `/api/plans/{id}/draft` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/plans/{id}/members`

Source: [`api/src/routes/plans/plans_id_members.rs`](../../api/src/routes/plans/plans_id_members.rs)

> /api/plans/{id}/members.
> Multiplayer plan membership + presence.
>   GET    → { members, active } — any member; active = user ids seen in the
>            last minute (Redis presence keys, 60s TTL).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members, active}` | 200, 404 | — |
| POST | `session` | [body](#post-apiplansidmembers-body) | `{members}` | 200, 400, 403 | — |
| PUT | `session` | — | `{ok}` | 200, 404 | — |
| DELETE | `session` | [body](#delete-apiplansidmembers-body) | `{members}` | 200, 400, 403 | — |

### POST `/api/plans/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |

### DELETE `/api/plans/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |


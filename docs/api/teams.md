# API reference — teams

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

3 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/teams`](#apiteams) | GET | `dual` |
| [`/api/teams`](#apiteams) | POST | `session` |
| [`/api/teams/{id}`](#apiteamsid) | PATCH | `session` |
| [`/api/teams/{id}`](#apiteamsid) | DELETE | `session` |
| [`/api/teams/{id}/members`](#apiteamsidmembers) | GET | `session` |
| [`/api/teams/{id}/members`](#apiteamsidmembers) | POST | `session` |
| [`/api/teams/{id}/members`](#apiteamsidmembers) | DELETE | `session` |

## `/api/teams`

Source: [`api/src/routes/teams/teams.rs`](../../api/src/routes/teams/teams.rs)

> /api/teams. GET → the caller's teams, resolved through ACTING user (a
> personal assistant acts as its owner — the identity-proxy model);
> POST { name } → create (humans only: requireUser).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{teams}` | 200, 401 | — |
| POST | `session` | [body](#post-apiteams-body) | `{team}` | 200, 400 | — |

### POST `/api/teams` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 120)` |  |

## `/api/teams/{id}`

Source: [`api/src/routes/teams/teams_id.rs`](../../api/src/routes/teams/teams_id.rs)

> /api/teams/{id}. PATCH { name } → rename (owner); DELETE → delete (owner)
> — the member rows cascade and its boards survive as personal boards
> (team_id set null, not cascaded), which is why both are owner-gated. A
> non-uuid {id} → the house 500. Gate order: uuid bind, then the owner
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apiteamsid-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403 | audit |

### PATCH `/api/teams/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 120)` |  |

## `/api/teams/{id}/members`

Source: [`api/src/routes/teams/teams_id_members.rs`](../../api/src/routes/teams/teams_id_members.rs)

> /api/teams/{id}/members. GET → members (any member of the team).
> POST { email, role? } → add (owner; the role defaults to 'member', and the
> email rides the audit row exactly as sent). DELETE { userId } → remove
> (owner; owners are silently kept by the SQL's role guard). Non-uuid {id} →
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members}` | 200, 403 | — |
| POST | `session` | [body](#post-apiteamsidmembers-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | [body](#delete-apiteamsidmembers-body) | `{ok}` | 200, 400, 403 | audit |

### POST `/api/teams/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |
| `role` | `enum(owner|member)` |  |

### DELETE `/api/teams/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |


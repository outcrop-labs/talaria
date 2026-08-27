# API reference — teams

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

Source: [`ui/src/routes/api/teams.ts`](../../ui/src/routes/api/teams.ts)

> GET → the user's teams (humans, or a personal assistant acting as its owner).
> POST { name } → create a team (user becomes owner; humans only).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{teams}` | 200, 401 | — |
| POST | `session` | [body](#post-apiteams-body) | `{team}` | 200 | — |

### POST `/api/teams` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(120)` |  |

## `/api/teams/{id}`

Source: [`ui/src/routes/api/teams.$id.ts`](../../ui/src/routes/api/teams.$id.ts)

> PATCH { name } → rename the team (owner). DELETE → delete it (owner); the
> member rows cascade and its boards survive as personal boards (team_id is
> set null, not cascaded), which is why this is owner-gated like every team
> mutation and not merely member-gated like the member read.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apiteamsid-body) | `{ok}` | 200, 403 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403 | audit |

### PATCH `/api/teams/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(120)` |  |

## `/api/teams/{id}/members`

Source: [`ui/src/routes/api/teams.$id.members.ts`](../../ui/src/routes/api/teams.$id.members.ts)

> GET → members (any member). POST { email, role } → add (owner). DELETE { userId } → remove (owner).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members}` | 200, 403 | — |
| POST | `session` | [body](#post-apiteamsidmembers-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | [body](#delete-apiteamsidmembers-body) | `{ok}` | 200, 403 | audit |

### POST `/api/teams/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `Email` |  |
| `role` | `z.enum(['owner', 'member']).default('member')` |  |

### DELETE `/api/teams/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |


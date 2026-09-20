# API reference — teams

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

6 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/teams`](#apiteams) | GET | `dual` |
| [`/api/teams`](#apiteams) | POST | `session` |
| [`/api/teams/{id}`](#apiteamsid) | GET | `session` |
| [`/api/teams/{id}`](#apiteamsid) | PATCH | `session` |
| [`/api/teams/{id}`](#apiteamsid) | DELETE | `session` |
| [`/api/teams/{id}/access`](#apiteamsidaccess) | GET | `admin` |
| [`/api/teams/{id}/access`](#apiteamsidaccess) | PUT | `admin` |
| [`/api/teams/{id}/agents`](#apiteamsidagents) | GET | `session` |
| [`/api/teams/{id}/agents`](#apiteamsidagents) | POST | `session` |
| [`/api/teams/{id}/agents`](#apiteamsidagents) | DELETE | `session` |
| [`/api/teams/{id}/members`](#apiteamsidmembers) | GET | `session` |
| [`/api/teams/{id}/members`](#apiteamsidmembers) | POST | `session` |
| [`/api/teams/{id}/members`](#apiteamsidmembers) | DELETE | `session` |
| [`/api/teams/directory`](#apiteamsdirectory) | GET | `dual` |

## `/api/teams`

Source: [`api/crates/talaria-routes-integrations/src/teams/teams.rs`](../../api/crates/talaria-routes-integrations/src/teams/teams.rs)

> /api/teams. GET → the caller's teams, resolved through ACTING user (a
> personal assistant acts as its owner — the identity-proxy model);
> GET ?all=1 → every org team (Manage view). POST { name } → create
> (humans only: requireUser).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{teams}` | 200, 401 | — |
| POST | `session` | — | `{team}` | 200, 400 | — |

## `/api/teams/{id}`

Source: [`api/crates/talaria-routes-integrations/src/teams/teams_id.rs`](../../api/crates/talaria-routes-integrations/src/teams/teams_id.rs)

> /api/teams/{id}. GET → team + members + agents (member, or Manage → Teams).
> PATCH { name?, description? } → rename / set blurb (owner); DELETE → delete
> (owner) — the member rows cascade and its boards survive as personal boards
> (team_id set null, not cascaded), which is why both are owner-gated. A
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{team, members, agents}` | 200, 404 | — |
| PATCH | `session` | [body](#patch-apiteamsid-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403 | audit |

### PATCH `/api/teams/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 120)` |  |
| `description` | `string? nullable` |  |

## `/api/teams/{id}/access`

Source: [`api/crates/talaria-routes-integrations/src/teams/teams_id_access.rs`](../../api/crates/talaria-routes-integrations/src/teams/teams_id_access.rs)

> /api/teams/{id}/access. GET/PUT the team's platform views and permission
> overrides — admin-only, same privilege as Admin → People. A team grant
> expands to its human members at resolution time.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `admin` | — | `{access}` | 200, 404 | — |
| PUT | `admin` | [body](#put-apiteamsidaccess-body) | `{access}` | 200, 400, 404 | audit |

### PUT `/api/teams/{id}/access` body

| field | schema | notes |
| :--- | :--- | :--- |
| `deniedViews` | `string[]?(1, 60, 40)` |  |
| `allowedManageViews` | `string[]?(1, 60, 10)` |  |

## `/api/teams/{id}/agents`

Source: [`api/crates/talaria-routes-integrations/src/teams/teams_id_agents.rs`](../../api/crates/talaria-routes-integrations/src/teams/teams_id_agents.rs)

> /api/teams/{id}/agents. GET → agent members (any team member).
> POST { model } → add (owner). DELETE { model } → remove (owner).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{agents}` | 200 | — |
| POST | `session` | [body](#post-apiteamsidagents-body) | `{ok}` | 200, 400, 403 | audit |
| DELETE | `session` | [body](#delete-apiteamsidagents-body) | `{ok}` | 200, 400, 403 | audit |

### POST `/api/teams/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `string(1, 200)` |  |

### DELETE `/api/teams/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `string(1, 200)` |  |

## `/api/teams/{id}/members`

Source: [`api/crates/talaria-routes-integrations/src/teams/teams_id_members.rs`](../../api/crates/talaria-routes-integrations/src/teams/teams_id_members.rs)

> /api/teams/{id}/members. GET → members (any member of the team).
> POST { email, role? } → add (owner; the role defaults to 'member', and the
> email rides the audit row exactly as sent). DELETE { userId } → remove
> (owner; owners are silently kept by the SQL's role guard). Non-uuid {id} →
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members}` | 200 | — |
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

## `/api/teams/directory`

Source: [`api/crates/talaria-routes-integrations/src/teams/teams_directory.rs`](../../api/crates/talaria-routes-integrations/src/teams/teams_directory.rs)

> /api/teams/directory. GET → every org team as id/name/counts, for share
> pickers. Any signed-in human (or identity-proxied assistant). Membership
> details stay on the member-gated routes.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{teams}` | 200, 401 | — |


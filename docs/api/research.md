# API reference — research

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

5 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/research`](#apiresearch) | GET | `dual` |
| [`/api/research`](#apiresearch) | POST | `dual` |
| [`/api/research/{id}`](#apiresearchid) | GET | `dual` |
| [`/api/research/{id}`](#apiresearchid) | DELETE | `session` |
| [`/api/research/{id}/conversation`](#apiresearchidconversation) | POST | `session` |
| [`/api/research/{id}/decide`](#apiresearchiddecide) | POST | `session` |
| [`/api/research/{id}/members`](#apiresearchidmembers) | GET | `session` |
| [`/api/research/{id}/members`](#apiresearchidmembers) | POST | `session` |
| [`/api/research/{id}/members`](#apiresearchidmembers) | DELETE | `session` |

## `/api/research`

Source: [`api/src/routes/research/research.rs`](../../api/src/routes/research/research.rs)

> /api/research.
> GET → recent runs scoped to the viewer + the mode catalog. POST { question,
> mode?, agentModel? } → start a run. Humans and agents both start research;
> an agent researches AS ITSELF, pinned to its own model.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{runs, modes}` | 200 | — |
| POST | `dual` | [body](#post-apiresearch-body) | `{run}` | 200, 400, 403, 409 | — |

### POST `/api/research` body

| field | schema | notes |
| :--- | :--- | :--- |
| `question` | `string(8, 4000)` |  |
| `agentModel` | `string?(200)` |  |
| `mode` | `enum(recon|brief|expedition)` |  |

## `/api/research/{id}`

Source: [`api/src/routes/research/research_id.rs`](../../api/src/routes/research/research_id.rs)

> /api/research/{id}.
> GET → one run + its citation registry (owner / shared member / org runs).
> DELETE → owner/admin, cancelling the run first so the driver stops
> spending on a report nobody will open.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{run, sources}` | 200, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

## `/api/research/{id}/conversation`

Source: [`api/src/routes/research/research_id_conversation.rs`](../../api/src/routes/research/research_id_conversation.rs)

> /api/research/{id}/conversation.
>
> OPEN THE CONVERSATION FOR A RUN, creating it the first time.
>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{conversationId}` | 200, 404, 409 | — |

## `/api/research/{id}/decide`

Source: [`api/src/routes/research/research_id_decide.rs`](../../api/src/routes/research/research_id_decide.rs)

> /api/research/{id}/decide.
>
> THE EXIT FROM 'awaiting', on the run's own surface. A parked run is an
> approval (runs::decide files it with the approvals machinery), and the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiresearchiddecide-body) | `{ok, status, phase}` | 200, 400, 403, 404, 409 | — |

### POST `/api/research/{id}/decide` body

| field | schema | notes |
| :--- | :--- | :--- |
| `optionId` | `string(1, 200)` |  |
| `note` | `string?(2000)` |  |

## `/api/research/{id}/members`

Source: [`api/src/routes/research/research_id_members.rs`](../../api/src/routes/research/research_id_members.rs)

> /api/research/{id}/members.
> Multiplayer research, mirroring plan membership. GET → members (any member).
> POST { email } → share (owner only; grants the report, notifies). DELETE
> { userId } → unshare (owner, or a collaborator leaving).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members}` | 200, 404 | — |
| POST | `session` | [body](#post-apiresearchidmembers-body) | `{members}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiresearchidmembers-body) | `{members}` | 200, 400, 403 | — |

### POST `/api/research/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |

### DELETE `/api/research/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |


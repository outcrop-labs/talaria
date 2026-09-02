# API reference — research

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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

Source: [`api/src/routes/research.rs`](../../api/src/routes/research.rs)

> GET → recent research runs (org-visible: research is shared knowledge) +
> the mode catalog. POST { question, mode, agentModel? } → start a run.
> Humans and agents (fleet key) both start runs; an agent researches AS
> ITSELF, and its owner (for a personal assistant) gets the notification.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{runs, modes}` | 200 | — |
| POST | `dual` | [body](#post-apiresearch-body) | `{run, duplicateOf}` | 200, 400, 403, 409 | — |

### POST `/api/research` body

| field | schema | notes |
| :--- | :--- | :--- |
| `question` | `z.string().min(8).max(4000)` |  |
| `mode` | `z.enum(['recon', 'brief', 'expedition']).default('brief')` |  |
| `agentModel` | `z.string().min(1).max(200).optional()` |  |

## `/api/research/{id}`

Source: [`api/src/routes/research_id.rs`](../../api/src/routes/research_id.rs)

> GET → one run + its citation registry (owner / shared member / org runs).
> DELETE → owner/admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

## `/api/research/{id}/conversation`

Source: [`api/src/routes/research_id_conversation.rs`](../../api/src/routes/research_id_conversation.rs)

> OPEN THE CONVERSATION FOR A RUN, creating it the first time.
>
> ON DEMAND, and that is the whole reason this is a POST rather than a field
> that always exists. Most research runs are read once and never discussed; a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `…` | 200, 404, 409 | — |

## `/api/research/{id}/decide`

Source: [`api/src/routes/research_id_decide.rs`](../../api/src/routes/research_id_decide.rs)

> THE EXIT FROM 'awaiting', on the run's own surface. A parked run is an
> approval (runs/decide.ts files it with the approvals machinery), and the
> research view is where the person it asked is already looking — the question
> renders in place via the projection's `awaiting` field, and this is the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiresearchiddecide-body) | `{ok, status, phase}` | 200, 400, 403, 404, 409 + varies | — |

### POST `/api/research/{id}/decide` body

| field | schema | notes |
| :--- | :--- | :--- |
| `optionId` | `z.string().min(1).max(200)` |  |
| `note` | `z.string().max(2000).optional()` |  |

## `/api/research/{id}/members`

Source: [`api/src/routes/research_id_members.rs`](../../api/src/routes/research_id_members.rs)

> Multiplayer research, mirroring plan membership. GET → members (any member).
> POST { email } → share (owner only; grants the report, notifies). DELETE
> { userId } → unshare (owner, or a collaborator leaving).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members}` | 200, 404 | — |
| POST | `session` | [body](#post-apiresearchidmembers-body) | `{members}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiresearchidmembers-body) | `{members}` | 200, 403 | — |

### POST `/api/research/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `Email` |  |

### DELETE `/api/research/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |


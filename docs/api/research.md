# API reference — research

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

4 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/research`](#apiresearch) | GET | `dual` |
| [`/api/research/{id}`](#apiresearchid) | GET | `dual` |
| [`/api/research/{id}/conversation`](#apiresearchidconversation) | POST | `session` |
| [`/api/research/{id}/members`](#apiresearchidmembers) | GET | `session` |
| [`/api/research/{id}/members`](#apiresearchidmembers) | POST | `session` |
| [`/api/research/{id}/members`](#apiresearchidmembers) | DELETE | `session` |

## `/api/research`

Source: [`ui/src/routes/api/research.ts`](../../ui/src/routes/api/research.ts)

> GET → recent research runs (org-visible: research is shared knowledge) +
> the mode catalog. POST { question, mode, agentModel? } → start a run.
> Humans and agents (fleet key) both start runs; an agent researches AS
> ITSELF, and its owner (for a personal assistant) gets the notification.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{runs, modes}` | 200 | — |

## `/api/research/{id}`

Source: [`ui/src/routes/api/research.$id.ts`](../../ui/src/routes/api/research.$id.ts)

> GET → one run + its citation registry (owner / shared member / org runs).
> DELETE → owner/admin.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{ok}` | 200, 403, 404 | — |

## `/api/research/{id}/conversation`

Source: [`ui/src/routes/api/research.$id.conversation.ts`](../../ui/src/routes/api/research.$id.conversation.ts)

> OPEN THE CONVERSATION FOR A RUN, creating it the first time.
>
> ON DEMAND, and that is the whole reason this is a POST rather than a field
> that always exists. Most research runs are read once and never discussed; a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `…` | 200, 404, 409 | — |

## `/api/research/{id}/members`

Source: [`ui/src/routes/api/research.$id.members.ts`](../../ui/src/routes/api/research.$id.members.ts)

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


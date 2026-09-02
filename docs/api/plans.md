# API reference — plans

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
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
| [`/api/plans/{id}/members`](#apiplansidmembers) | DELETE | `session` |
| [`/api/plans/{id}/members`](#apiplansidmembers) | PUT | `session` |

## `/api/plans/{id}/doc`

Source: [`api/src/routes/plans_id_doc.rs`](../../api/src/routes/plans_id_doc.rs)

> The plan's living document (a linked doc artifact). GET → find-or-create it,
> seeded from the agent's plan template when one is bound. POST → the plan's
> agent rewrites it from the conversation so far. Owner or plan collaborator;
> the document is always OWNED by the plan's owner regardless of who touched
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 404 | — |
| POST | `session` | [body](#post-apiplansiddoc-body) | `{artifactId}` | 200, 403, 404, 502 | — |

### POST `/api/plans/{id}/doc` body

| field | schema | notes |
| :--- | :--- | :--- |
| `tier` | `z.string().max(60).nullish()` |  |

## `/api/plans/{id}/draft`

Source: [`api/src/routes/plans_id_draft.rs`](../../api/src/routes/plans_id_draft.rs)

> The plan's ticket drafts, as a DURABLE JOB: POST enqueues a 'plan-draft' run
> and answers immediately with the queued draft; the agent reads the
> conversation server-side, so drafting survives a closed tab and a restarted
> server alike. GET is the review's way back to the conversation's latest
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{draft}` | 200, 404 | — |
| POST | `session` | [body](#post-apiplansiddraft-body) | `…` | 200, 403, 404, 500 | — |
| PATCH | `session` | [body](#patch-apiplansiddraft-body) | `{ok}` | 200, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 404 | — |

### POST `/api/plans/{id}/draft` body

| field | schema | notes |
| :--- | :--- | :--- |
| `tier` | `z.string().max(60).nullish()` |  |
| `boardId` | `Uuid.nullish()` |  |
| `templateId` | `Uuid.nullish()` |  |

### PATCH `/api/plans/{id}/draft` body

| field | schema | notes |
| :--- | :--- | :--- |
| `proposals` | `z.array(z.object({ title: z.string().max(500), description: z.string().max(20_000), priority: z.enum(['low', 'medium', 'high', 'urgent']), …` |  |

## `/api/plans/{id}/members`

Source: [`api/src/routes/plans_id_members.rs`](../../api/src/routes/plans_id_members.rs)

> Multiplayer plan membership + presence.
> GET → { members, active } (any member; active = user ids seen in the last
> minute). POST { email } → share (owner only; grants the doc, notifies them).
> DELETE { userId } → unshare (owner, or a collaborator leaving). PUT → presence
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{active}` | 200, 404 | — |
| POST | `session` | [body](#post-apiplansidmembers-body) | `{members}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiplansidmembers-body) | `{members}` | 200, 403 | — |
| PUT | `session` | — | `{ok}` | 200, 404 | — |

### POST `/api/plans/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `Email` |  |

### DELETE `/api/plans/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |


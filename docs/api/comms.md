# API reference — comms

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

15 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/channels`](#apichannels) | GET | `dual` |
| [`/api/channels`](#apichannels) | POST | `session` |
| [`/api/channels/{id}`](#apichannelsid) | GET | `session` |
| [`/api/channels/{id}`](#apichannelsid) | PUT | `session` |
| [`/api/channels/{id}`](#apichannelsid) | DELETE | `session` |
| [`/api/channels/{id}/agents`](#apichannelsidagents) | POST | `session` |
| [`/api/channels/{id}/agents`](#apichannelsidagents) | DELETE | `session` |
| [`/api/channels/{id}/conclude`](#apichannelsidconclude) | POST | `session` |
| [`/api/channels/{id}/events`](#apichannelsidevents) | GET | `session` |
| [`/api/channels/{id}/members`](#apichannelsidmembers) | POST | `session` |
| [`/api/channels/{id}/members`](#apichannelsidmembers) | DELETE | `session` |
| [`/api/channels/{id}/messages`](#apichannelsidmessages) | GET | `dual` |
| [`/api/channels/{id}/messages`](#apichannelsidmessages) | POST | `dual` |
| [`/api/channels/{id}/messages/{msgId}`](#apichannelsidmessagesmsgid) | PATCH | `session` |
| [`/api/channels/{id}/messages/{msgId}`](#apichannelsidmessagesmsgid) | DELETE | `session` |
| [`/api/channels/{id}/messages/{msgId}/reactions`](#apichannelsidmessagesmsgidreactions) | POST | `dual` |
| [`/api/channels/{id}/plan`](#apichannelsidplan) | GET | `session` |
| [`/api/channels/{id}/plan`](#apichannelsidplan) | POST | `session` |
| [`/api/channels/{id}/plan`](#apichannelsidplan) | PATCH | `session` |
| [`/api/channels/{id}/plan`](#apichannelsidplan) | DELETE | `session` |
| [`/api/channels/{id}/read`](#apichannelsidread) | POST | `session` |
| [`/api/chat`](#apichat) | POST | `session` + `perm:plans.create` |
| [`/api/conversations`](#apiconversations) | GET | `session` |
| [`/api/conversations/{id}`](#apiconversationsid) | GET | `session` |
| [`/api/conversations/{id}`](#apiconversationsid) | PATCH | `session` |
| [`/api/dms`](#apidms) | POST | `session` |

## `/api/channels`

Source: [`api/src/routes/channels.rs`](../../api/src/routes/channels.rs)

> GET /api/channels → the user's channels/relays/DMs. POST { name, topic?,
> kind? } → create a channel (default) or a Relay (kind 'group').

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{channels}` | 200 | — |
| POST | `session` | [body](#post-apichannels-body) | `{channel}` | 200, 403 | — |

### POST `/api/channels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80)` |  |
| `topic` | `z.string().max(300).nullish()` |  |
| `kind` | `z.enum(['channel', 'group']).optional()` |  |

## `/api/channels/{id}`

Source: [`api/src/routes/channels_id.rs`](../../api/src/routes/channels_id.rs)

> GET → channel detail (members + agents). PUT → rename / set topic (owner).
> DELETE → archive (?hard=1 deletes; owner only).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members, agents}` | 200, 403 | — |
| PUT | `session` | [body](#put-apichannelsid-body) | `{ok}` | 200, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PUT `/api/channels/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80).optional()` |  |
| `topic` | `z.string().max(300).nullish()` |  |

## `/api/channels/{id}/agents`

Source: [`api/src/routes/channels_id_agents.rs`](../../api/src/routes/channels_id_agents.rs)

> POST { model } → add a fleet agent to the channel (adder needs access to that
> agent). DELETE { model } → remove it. Any member.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidagents-body) | `{ok}` | 200, 403 | — |
| DELETE | `session` | [body](#delete-apichannelsidagents-body) | `{ok}` | 200, 403 | — |

### POST `/api/channels/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `z.string().min(1).max(200)` |  |

### DELETE `/api/channels/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `z.string().min(1).max(200)` |  |

## `/api/channels/{id}/conclude`

Source: [`api/src/routes/channels_id_conclude.rs`](../../api/src/routes/channels_id_conclude.rs)

> POST → conclude a Relay: summarize what was decided (posted as the final
> message + indexed for retrieval), then archive it. Members only; relays only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{summary}` | 200, 400, 403, 404, 502 | — |

## `/api/channels/{id}/events`

Source: [`api/src/routes/channels_id_events.rs`](../../api/src/routes/channels_id_events.rs)

> GET /api/channels/:id/events → SSE stream of the channel's live events
> (messages, membership). Auth-gated to members. Powers multiplayer chat.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/channels/{id}/members`

Source: [`api/src/routes/channels_id_members.rs`](../../api/src/routes/channels_id_members.rs)

> POST { email } → add a member (any member can invite).
> DELETE { userId } → remove a member (owner, or yourself to leave).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidmembers-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apichannelsidmembers-body) | `{ok}` | 200, 403 | — |

### POST `/api/channels/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `Email` |  |

### DELETE `/api/channels/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |

## `/api/channels/{id}/messages`

Source: [`api/src/routes/channels_id_messages.rs`](../../api/src/routes/channels_id_messages.rs)

> GET ?since=<seq> → the channel's messages (members). POST { content } → post
> a message; @mentioned channel agents reply, streamed into the channel.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{messages}` | 200, 403 | — |
| POST | `dual` | [body](#post-apichannelsidmessages-body) | `{message}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/messages` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `z.string().max(20_000).default('')` |  |
| `attachmentIds` | `z.array(Uuid).max(10).optional()` |  |
| `refs` | `z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: Uuid })).max(3).optional()` |  |
| `threadRootId` | `Uuid.nullish()` |  |

## `/api/channels/{id}/messages/{msgId}`

Source: [`api/src/routes/channels_id_messages_msgid.rs`](../../api/src/routes/channels_id_messages_msgid.rs)

> PATCH { content } → edit your own message (edited marker shows).
> DELETE → remove it: the author, or the channel owner tidying up. A thread
> root takes its replies with it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apichannelsidmessagesmsgid-body) | `{ok}` | 200, 403, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

### PATCH `/api/channels/{id}/messages/{msgId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `z.string().trim().min(1).max(20_000)` |  |

## `/api/channels/{id}/messages/{msgId}/reactions`

Source: [`api/src/routes/channels_id_messages_msgid_reactions.rs`](../../api/src/routes/channels_id_messages_msgid_reactions.rs)

> POST { emoji } → toggle your reaction on a message. Agents react too, under
> their own identity — one of our twists on the Slack shape.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apichannelsidmessagesmsgidreactions-body) | `{ok}` | 200, 403, 404 | — |

### POST `/api/channels/{id}/messages/{msgId}/reactions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `emoji` | `z.string().min(1).max(16)` |  |

## `/api/channels/{id}/plan`

Source: [`api/src/routes/channels_id_plan.rs`](../../api/src/routes/channels_id_plan.rs)

> Channel ticket drafts, as a DURABLE JOB (the plan surface's twin — one
> domain module, two conversation kinds). POST enqueues a 'plan-draft' run on
> a channel agent and answers immediately with the queued draft; GET/PATCH/
> DELETE read, persist edits to, and drop the channel's latest draft. Members
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{draft}` | 200, 403 | — |
| POST | `session` | [body](#post-apichannelsidplan-body) | `…` | 200, 400, 403, 500 | — |
| PATCH | `session` | [body](#patch-apichannelsidplan-body) | `{ok}` | 200, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### POST `/api/channels/{id}/plan` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentModel` | `z.string().min(1).max(200)` |  |
| `tier` | `z.string().max(60).nullish()` |  |
| `boardId` | `Uuid.nullish()` |  |
| `templateId` | `Uuid.nullish()` |  |

### PATCH `/api/channels/{id}/plan` body

| field | schema | notes |
| :--- | :--- | :--- |
| `proposals` | `z.array(z.object({ title: z.string().max(500), description: z.string().max(20_000), priority: z.enum(['low', 'medium', 'high', 'urgent']), …` |  |

## `/api/channels/{id}/read`

Source: [`api/src/routes/channels_id_read.rs`](../../api/src/routes/channels_id_read.rs)

> POST { seq } → advance the caller's read cursor (drives unread badges).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidread-body) | `{ok}` | 200, 403 | — |

### POST `/api/channels/{id}/read` body

| field | schema | notes |
| :--- | :--- | :--- |
| `seq` | `z.number().int().min(0)` |  |

## `/api/chat`

Source: [`api/src/routes/chat.rs`](../../api/src/routes/chat.rs)

> POST /api/chat { model, conversationId?, content } → durable streaming chat.
> Persists the turn to Postgres (server owns history) and tees the gateway
> stream: one branch to the client, one drained to the DB so an in-progress
> reply survives a disconnect. Returns SSE + X-Conversation-Id / X-Message-Id.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:plans.create` | [body](#post-apichat-body) | `{queued, conversationId}` | 200, 202, 400, 403, 404 + varies | SSE |

### POST `/api/chat` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `z.string().min(1)` |  |
| `conversationId` | `Uuid.optional()` |  |
| `content` | `z.string().max(100_000).default('')` |  |
| `tier` | `z.string().max(60).optional()` |  |
| `effort` | `z.string().max(24).optional()` |  |
| `attachmentIds` | `z.array(Uuid).max(10).optional()` |  |
| `refs` | `z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: Uuid })).max(3).optional()` |  |
| `kind` | `z.enum(['chat', 'plan', 'research']).optional()` |  |
| `templateId` | `Uuid.nullish()` |  |
| `queue` | `z.boolean().optional()` |  |

## `/api/conversations`

Source: [`api/src/routes/conversations.rs`](../../api/src/routes/conversations.rs)

> GET /api/conversations → the current user's conversations (newest first).
> The client groups them by agent.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{conversations}` | 200 | — |

## `/api/conversations/{id}`

Source: [`api/src/routes/conversations_id.rs`](../../api/src/routes/conversations_id.rs)

> GET /api/conversations/:id → a conversation + its messages (ownership-checked).
> PATCH { title } → rename (owner, or a plan collaborator). A renamed title no
> longer matches the mechanical first-message truncation, so the Titler and
> its sweep leave it alone from then on.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 404 | — |
| PATCH | `session` | [body](#patch-apiconversationsid-body) | `{ok}` | 200, 404 | — |

### PATCH `/api/conversations/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().trim().min(1).max(120)` |  |

## `/api/dms`

Source: [`api/src/routes/dms.rs`](../../api/src/routes/dms.rs)

> POST { userId } → find-or-create the DM with that person (rides the channel
> machinery: same messages, SSE feed, and composer as everything else).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apidms-body) | `{channel}` | 200, 400 | — |

### POST `/api/dms` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `Uuid` |  |


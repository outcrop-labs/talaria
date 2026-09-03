# API reference — comms

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

16 routes.

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
| [`/api/conversations/{id}/read`](#apiconversationsidread) | POST | `session` |
| [`/api/dms`](#apidms) | POST | `session` |

## `/api/channels`

Source: [`api/src/routes/comms/channels.rs`](../../api/src/routes/comms/channels.rs)

> /api/channels.
> GET → the user's channels/relays/DMs (agents see the channels they've been
> added to, elevated assistants every non-DM). POST { name, topic?, kind? } →
> create a channel (default) or a Relay (kind 'group'), behind the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{channels}` | 200 | — |
| POST | `session` | [body](#post-apichannels-body) | `{channel}` | 200, 400, 403 | — |

### POST `/api/channels` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 80)` |  |
| `topic` | `string? nullable(300)` |  |
| `kind` | `enum(channel|group)?` |  |

## `/api/channels/{id}`

Source: [`api/src/routes/comms/channels_id.rs`](../../api/src/routes/comms/channels_id.rs)

> /api/channels/{id}.
> GET → channel detail (role + members + agents). PUT → rename / set topic
> (owner). DELETE → archive (?hard=1 deletes; owner only; a hard delete also
> purges the channel's activity points so nothing orphans in the index).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{role, members, agents}` | 200, 403 | — |
| PUT | `session` | [body](#put-apichannelsid-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PUT `/api/channels/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(80)` | name: min 1, max 80, optional — the empty name is a min failure, not a value (unlike the topic below). |
| `topic` | `string? nullable(300)` | Three states, not two (max 300): absent leaves the topic alone, present-null clears it, a string sets it. |

## `/api/channels/{id}/agents`

Source: [`api/src/routes/comms/channels_id_agents.rs`](../../api/src/routes/comms/channels_id_agents.rs)

> /api/channels/{id}/agents.
> POST { model } → add a fleet agent to the channel (the adder needs access
> to that agent). DELETE { model } → remove it. Any member.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidagents-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apichannelsidagents-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `string(1, 200)` |  |

### DELETE `/api/channels/{id}/agents` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `string(1, 200)` |  |

## `/api/channels/{id}/conclude`

Source: [`api/src/routes/comms/channels_id_conclude.rs`](../../api/src/routes/comms/channels_id_conclude.rs)

> /api/channels/{id}/conclude.
> POST → conclude a Relay: summarize what was decided (posted as the final
> message + indexed for retrieval), then archive it. Members only; relays
> only — channels persist. The summarize failures surface as 502 with
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{summary}` | 200, 400, 403, 404, 502 | — |

## `/api/channels/{id}/events`

Source: [`api/src/routes/comms/channels_id_events.rs`](../../api/src/routes/comms/channels_id_events.rs)

> /api/channels/{id}/events.
> SSE stream of the channel's live events (messages, membership),
> auth-gated to members. Powers multiplayer chat. The stream itself is
> realtime's (channel:<id> topic); this route is only the gate in front of it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/channels/{id}/members`

Source: [`api/src/routes/comms/channels_id_members.rs`](../../api/src/routes/comms/channels_id_members.rs)

> /api/channels/{id}/members.
> POST { email } → add a member (any member can invite; they must have
> signed in before — the engine answers the no-show sentence as a 400).
> DELETE { userId } → remove a member (owner, or yourself to leave).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidmembers-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apichannelsidmembers-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |

### DELETE `/api/channels/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |

## `/api/channels/{id}/messages`

Source: [`api/src/routes/comms/channels_id_messages.rs`](../../api/src/routes/comms/channels_id_messages.rs)

> /api/channels/{id}/messages.
> GET ?since=<seq>&thread=<id> → the channel's messages (members; agents in
> the channel, elevated assistants any non-DM). POST { content } → post a
> message; @mentioned channel agents reply, streamed into the channel.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{messages}` | 200, 403 | — |
| POST | `dual` | [body](#post-apichannelsidmessages-body) | `{message}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/messages` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string(0, 20000)` |  |
| `attachmentIds` | `uuid[]?(10)` |  |
| `threadRootId` | `uuid?` |  |

## `/api/channels/{id}/messages/{msgId}`

Source: [`api/src/routes/comms/channels_id_messages_msgid.rs`](../../api/src/routes/comms/channels_id_messages_msgid.rs)

> /api/channels/{id}/messages/{msgId}.
> PATCH { content } → edit your own message (edited marker shows).
> DELETE → remove it: the author, or the channel owner tidying up. A thread
> root takes its replies with it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apichannelsidmessagesmsgid-body) | `{ok}` | 200, 400, 403, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

### PATCH `/api/channels/{id}/messages/{msgId}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string trimmed(1, 20000)` |  |

## `/api/channels/{id}/messages/{msgId}/reactions`

Source: [`api/src/routes/comms/channels_id_messages_msgid_reactions.rs`](../../api/src/routes/comms/channels_id_messages_msgid_reactions.rs)

> /api/channels/{id}/messages/{msgId}/reactions.
> POST { emoji } → toggle your reaction on a message. Agents react too, under
> their own identity — one of our twists on the Slack shape.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apichannelsidmessagesmsgidreactions-body) | `{ok}` | 200, 400, 403, 404 | — |

### POST `/api/channels/{id}/messages/{msgId}/reactions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `emoji` | `string(1, 16)` |  |

## `/api/channels/{id}/plan`

Source: [`api/src/routes/comms/channels_id_plan.rs`](../../api/src/routes/comms/channels_id_plan.rs)

> /api/channels/{id}/plan.
> The channel Plan button: POST enqueues a 'plan-draft' run on a channel
> agent and answers immediately with the queued draft; GET/PATCH/DELETE
> read, persist edits to, and drop the channel's latest draft. Members only;
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{draft}` | 200, 403 | — |
| POST | `session` | [body](#post-apichannelsidplan-body) | `{draft}` | 200, 400, 403, 500 | — |
| PATCH | `session` | [body](#patch-apichannelsidplan-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### POST `/api/channels/{id}/plan` body

| field | schema | notes |
| :--- | :--- | :--- |
| `agentModel` | `string(1, 200)` |  |
| `tier` | `string? nullish(60)` |  |
| `boardId` | `uuid?` |  |
| `templateId` | `uuid?` |  |

### PATCH `/api/channels/{id}/plan` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/channels/{id}/read`

Source: [`api/src/routes/comms/channels_id_read.rs`](../../api/src/routes/comms/channels_id_read.rs)

> /api/channels/{id}/read.
> POST { seq } → advance the caller's read cursor (drives unread badges).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidread-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/read` body

| field | schema | notes |
| :--- | :--- | :--- |
| `seq` | `number(0, 9007)` | seq: integer, min 0, no schema max — the ceiling is the safe-integer bound itself. |

## `/api/chat`

Source: [`api/src/routes/comms/chat.rs`](../../api/src/routes/comms/chat.rs)

> /api/chat. POST { model,
> conversationId?, content } → durable streaming chat: the turn is persisted
> to Postgres (the server owns history) and the gateway stream is TEED — one
> branch to the client as SSE with X-Conversation-Id / X-Message-Id, one
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:plans.create` | [body](#post-apichat-body) | `{role, content}` | 200, 202, 400, 403, 404, 500 | SSE |

### POST `/api/chat` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `string(1)` |  |
| `conversationId` | `uuid?` |  |
| `content` | `string(0, 100000)` |  |
| `tier` | `string?(60)` |  |
| `effort` | `string?(24)` |  |
| `attachmentIds` | `uuid[]?(10)` |  |
| `kind` | `enum(chat|plan|research)?` |  |
| `templateId` | `uuid?` |  |
| `queue` | `bool?` |  |

## `/api/conversations`

Source: [`api/src/routes/comms/conversations.rs`](../../api/src/routes/comms/conversations.rs)

> /api/conversations. GET
> ?kind=plan → the user's plan conversations; anything else → their chats.
> Newest activity first; the client groups them by agent.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/conversations/{id}`

Source: [`api/src/routes/comms/conversations_id.rs`](../../api/src/routes/comms/conversations_id.rs)

> /api/conversations/{id}.
> GET → the conversation + its messages (ownership-checked). PATCH { title }
> → rename (owner, or a plan collaborator). A renamed title no longer matches
> the mechanical first-message truncation, so the Titler and its sweep leave
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 404 | — |
| PATCH | `session` | [body](#patch-apiconversationsid-body) | `…` | 200, 400, 404 | — |

### PATCH `/api/conversations/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string trimmed(1, 120)` |  |

## `/api/conversations/{id}/read`

Source: [`api/src/routes/comms/conversations_id_read.rs`](../../api/src/routes/comms/conversations_id_read.rs)

> /api/conversations/{id}/read.
> POST { seq? } → advance the caller's read cursor (drives the thread's
> unread pill), and — when the advanced cursor covers the thread's latest
> turn — mark the bell rows pointing at the thread read (opening the thread
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiconversationsidread-body) | `{ok, cleared}` | 200, 400, 403 | — |

### POST `/api/conversations/{id}/read` body

| field | schema | notes |
| :--- | :--- | :--- |
| `seq` | `number?(0, 9007)` | seq: optional integer, min 0, no schema max — the ceiling is the safe-integer bound itself. |

## `/api/dms`

Source: [`api/src/routes/comms/dms.rs`](../../api/src/routes/comms/dms.rs)

> /api/dms. POST { userId } → find-or-create the DM with that person (rides
> the channel machinery: same messages, SSE feed, and composer as
> everything else).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apidms-body) | `…` | 200, 400 | — |

### POST `/api/dms` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |


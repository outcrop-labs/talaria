# API reference — comms

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

33 routes.

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
| [`/api/channels/{id}/teams`](#apichannelsidteams) | POST | `session` |
| [`/api/channels/{id}/teams`](#apichannelsidteams) | DELETE | `session` |
| [`/api/channels/{id}/typing`](#apichannelsidtyping) | POST | `session` |
| [`/api/chat`](#apichat) | POST | `session` |
| [`/api/chat/chips/approvals/{id}`](#apichatchipsapprovalsid) | POST | `session` |
| [`/api/chat/chips/resolve`](#apichatchipsresolve) | POST | `session` |
| [`/api/chat/turns/{id}/stop`](#apichatturnsidstop) | POST | `session` |
| [`/api/conversations`](#apiconversations) | GET | `session` |
| [`/api/conversations/{id}`](#apiconversationsid) | GET | `session` |
| [`/api/conversations/{id}`](#apiconversationsid) | PATCH | `session` |
| [`/api/conversations/{id}`](#apiconversationsid) | DELETE | `session` |
| [`/api/conversations/{id}/doc`](#apiconversationsiddoc) | GET | `session` |
| [`/api/conversations/{id}/doc`](#apiconversationsiddoc) | POST | `session` |
| [`/api/conversations/{id}/files`](#apiconversationsidfiles) | GET | `session` |
| [`/api/conversations/{id}/members`](#apiconversationsidmembers) | GET | `session` |
| [`/api/conversations/{id}/members`](#apiconversationsidmembers) | POST | `session` |
| [`/api/conversations/{id}/members`](#apiconversationsidmembers) | PUT | `session` |
| [`/api/conversations/{id}/members`](#apiconversationsidmembers) | DELETE | `session` |
| [`/api/conversations/{id}/messages/{msgId}/reactions`](#apiconversationsidmessagesmsgidreactions) | POST | `session` |
| [`/api/conversations/{id}/read`](#apiconversationsidread) | POST | `session` |
| [`/api/conversations/{id}/schedule`](#apiconversationsidschedule) | POST | `session` + `agent-manager` |
| [`/api/conversations/{id}/schedules`](#apiconversationsidschedules) | GET | `session` |
| [`/api/conversations/{id}/schedules/{jobId}`](#apiconversationsidschedulesjobid) | DELETE | `session` + `agent-manager` |
| [`/api/conversations/{id}/session`](#apiconversationsidsession) | GET | `session` |
| [`/api/conversations/{id}/steer`](#apiconversationsidsteer) | POST | `session` |
| [`/api/conversations/{id}/stop`](#apiconversationsidstop) | POST | `session` |
| [`/api/conversations/{id}/teams`](#apiconversationsidteams) | POST | `session` |
| [`/api/conversations/{id}/teams`](#apiconversationsidteams) | DELETE | `session` |
| [`/api/conversations/{id}/todo`](#apiconversationsidtodo) | GET | `session` |
| [`/api/dms`](#apidms) | POST | `session` |

## `/api/channels`

Source: [`api/crates/talaria-routes-comms/src/comms/channels.rs`](../../api/crates/talaria-routes-comms/src/comms/channels.rs)

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

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id.rs)

> /api/channels/{id}.
> GET → channel detail (role + members + agents + teams + agentInitiative).
> PUT → rename / set topic / set agentInitiative (owner). DELETE → archive (?hard=1 deletes; owner only; a hard delete also
> purges the channel's activity points so nothing orphans in the index).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{role, members, agents, teams, agentInitiative}` | 200, 403 | — |
| PUT | `session` | [body](#put-apichannelsid-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PUT `/api/channels/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string?(80)` | name: min 1, max 80, optional — the empty name is a min failure, not a value (unlike the topic below). |
| `topic` | `string? nullable(300)` | Three states, not two (max 300): absent leaves the topic alone, present-null clears it, a string sets it. |
| `agentInitiative` | `bool` |  |

## `/api/channels/{id}/agents`

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_agents.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_agents.rs)

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

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_conclude.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_conclude.rs)

> /api/channels/{id}/conclude.
> POST → conclude a Relay: summarize what was decided (posted as the final
> message + indexed for retrieval), then archive it. Members only; relays
> only — channels persist. The summarize failures surface as 502 with
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{summary}` | 200, 400, 403, 404, 502 | — |

## `/api/channels/{id}/events`

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_events.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_events.rs)

> /api/channels/{id}/events.
> SSE stream of the channel's live events (messages, membership),
> auth-gated to members. Powers multiplayer chat. The stream itself is
> realtime's (channel:<id> topic); this route is only the gate in front of it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/channels/{id}/members`

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_members.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_members.rs)

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

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_messages.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_messages.rs)

> /api/channels/{id}/messages.
> GET ?since=<seq>&thread=<id> → the channel's messages (members; agents in
> the channel, elevated assistants any non-DM). POST { content } → post a
> message; @mentioned channel agents reply, streamed into the channel.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{messages}` | 200, 400, 403 | — |
| POST | `dual` | [body](#post-apichannelsidmessages-body) | `{message}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/messages` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string(0, 20000)` |  |
| `attachmentIds` | `uuid[]?(10)` |  |
| `threadRootId` | `uuid?` |  |

## `/api/channels/{id}/messages/{msgId}`

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_messages_msgid.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_messages_msgid.rs)

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

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_messages_msgid_reactions.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_messages_msgid_reactions.rs)

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

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_plan.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_plan.rs)

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

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_read.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_read.rs)

> /api/channels/{id}/read.
> POST { seq } → advance the caller's read cursor (drives unread badges).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidread-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/read` body

| field | schema | notes |
| :--- | :--- | :--- |
| `seq` | `number(0, 9007)` | seq: integer, min 0, no schema max — the ceiling is the safe-integer bound itself. |

## `/api/channels/{id}/teams`

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_teams.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_teams.rs)

> /api/channels/{id}/teams.
> POST { teamId } → grant a team (any member). DELETE { teamId } → revoke
> (owner, or any member removing a team they belong to). Direct messages
> stay private.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidteams-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apichannelsidteams-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/teams` body

| field | schema | notes |
| :--- | :--- | :--- |
| `teamId` | `uuid` |  |

### DELETE `/api/channels/{id}/teams` body

| field | schema | notes |
| :--- | :--- | :--- |
| `teamId` | `uuid` |  |

## `/api/channels/{id}/typing`

Source: [`api/crates/talaria-routes-comms/src/comms/channels_id_typing.rs`](../../api/crates/talaria-routes-comms/src/comms/channels_id_typing.rs)

> /api/channels/{id}/typing.
> POST { typing?: boolean, threadRootId?: uuid } → tell the channel's other
> members that the caller is typing (default) or has stopped — in the channel
> itself, or in one thread when `threadRootId` is set. Ephemeral: nothing is
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichannelsidtyping-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/channels/{id}/typing` body

| field | schema | notes |
| :--- | :--- | :--- |
| `typing` | `bool?` |  |
| `threadRootId` | `uuid` |  |

## `/api/chat`

Source: [`api/crates/talaria-routes-comms/src/comms/chat.rs`](../../api/crates/talaria-routes-comms/src/comms/chat.rs)

> /api/chat. POST { model,
> conversationId?, content } → durable streaming chat: the turn is persisted
> to Postgres (the server owns history) and the gateway stream is TEED — one
> branch to the client as SSE with X-Conversation-Id / X-Message-Id, one
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichat-body) | `{role, content}` | 200, 202, 400, 403, 404, 409, … | SSE |

### POST `/api/chat` body

| field | schema | notes |
| :--- | :--- | :--- |
| `model` | `string(1)` |  |
| `conversationId` | `uuid?` |  |
| `content` | `string(0, 100000)` |  |
| `tier` | `string?(60)` |  |
| `effort` | `string?(24)` |  |
| `attachmentIds` | `uuid[]?(10)` |  |
| `kind` | `enum(chat|plan|research|ticket|work)?` |  |
| `templateId` | `uuid?` |  |
| `queue` | `bool?` |  |
| `focus` | `focus` | { kind: artifact|google, id, title? } or null — the file open in the teammate's pane; read on work turns only |
| `turnId` | `uuid?` |  |

## `/api/chat/chips/approvals/{id}`

Source: [`api/crates/talaria-routes-comms/src/comms/chips_approvals_id.rs`](../../api/crates/talaria-routes-comms/src/comms/chips_approvals_id.rs)

> POST /api/chat/chips/approvals/{id} — approve or deny a chip.
> Google sends execute through the existing confirm-send. Ticket moves apply
> as the person who approved. Either way the chip that follows names the
> tools that approval unlocked. An agent's own approval request (kind
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichatchipsapprovalsid-body) | `{status, tools}` | 200, 400, 403, 404, 409, 502 | — |

**POST** — Approve or deny a protected action surfaced as a chip in the thread.

### POST `/api/chat/chips/approvals/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `decision` | `enum(approve|reject)` |  |

## `/api/chat/chips/resolve`

Source: [`api/crates/talaria-routes-comms/src/comms/chips.rs`](../../api/crates/talaria-routes-comms/src/comms/chips.rs)

> POST /api/chat/chips/resolve — titles for platform links pasted in chat.
> A link the caller cannot read comes back without a title: the chip shows
> the kind, not the name.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apichatchipsresolve-body) | `{chips}` | 200, 400 | — |

**POST** — Titles for platform links pasted in chat. A link the caller cannot read comes back without a title — the chip shows the kind, not the name.

### POST `/api/chat/chips/resolve` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/chat/turns/{id}/stop`

Source: [`api/crates/talaria-routes-comms/src/comms/chat_turns_id_stop.rs`](../../api/crates/talaria-routes-comms/src/comms/chat_turns_id_stop.rs)

> /api/chat/turns/{id}/stop.
> POST → stop the turn the page's own send named `id` (`turnId` on
> /api/chat), wherever it has got to. The page sends this the moment Stop is
> pressed on a new thread's first turn, whose thread id rides back only with
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{ok, hermes, held}` | 200, 409 | — |

**POST** — Stop the turn the caller's own send named (`turnId` on /api/chat), before or after the api has heard of it: the stop a page sends for a new thread, whose id it does not have yet. Waits up to 10 s for the send, then stops its thread's turn as /api/conversations/{id}/stop does (`hermes`). A send that has not come by then gets the stop `held`, for its turn to be stopped when it opens. Only the sender's own turn: anything else, or a turn that ended → 409.

## `/api/conversations`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations.rs)

> /api/conversations. GET
> ?kind=plan → the user's plan conversations; ?kind=work → their work
> sessions; anything else → their chats.
> ?archived=1 → the retired set (exact string '1'); everything else is live.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/conversations/{id}`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id.rs)

> /api/conversations/{id}.
> GET → the conversation + its messages (ownership-checked). PATCH { title }
> → rename (owner, or a plan collaborator). A renamed title no longer matches
> the mechanical first-message truncation, so the Titler and its sweep leave
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 404 | — |
| PATCH | `session` | [body](#patch-apiconversationsid-body) | `…` | 200, 400, 403, 404 | — |
| DELETE | `session` | — | `…` | 200, 403, 404 | — |

**PATCH** — PATCH takes any of five writes. `title` (1..120, trimmed) renames the thread. `archived: false` restores one the owner archived — owner only, and archiving itself is DELETE. `pinnedFiles` replaces the document pane's list (at most 12 entries, each { kind: "google" | "artifact", id, title?, mime? }). And three fields that say where the CALLER keeps this thread rather than anything about the thread: `pinned` (bool), `group` (trimmed string, 40 characters at most) and `position` (whole number, 0 to 100000). On group and position, null is the explicit clear and an absent field is left alone; those three land in the caller's own layout row, so any role the GET would show may write them, collaborator included, while title and archived stay owner (or plan-collaborator) writes. A body naming none of the five is a 400.

### PATCH `/api/conversations/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string trimmed(1, 120)` |  |
| `archived` | `bool?` |  |

## `/api/conversations/{id}/doc`

Source: [`api/crates/talaria-routes-boards/src/plans/plans_id_doc.rs`](../../api/crates/talaria-routes-boards/src/plans/plans_id_doc.rs)

> /api/plans/{id}/doc.
> The plan's living document (a linked doc artifact). GET → find-or-create
> it, seeded from the agent's plan template when one is bound. POST → the
> plan's agent rewrites it from the conversation so far. Owner or plan
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{artifact}` | 200, 404 | — |
| POST | `session` | [body](#post-apiconversationsiddoc-body) | `{artifactId}` | 200, 400, 403, 404, 409, 502 | — |

**GET** — A plan: its living document, created (from the plan template) on the first read. A work session: the living document a legacy session already has, else 404 "this session has no plan document" — never creates one.

**POST** — A plan: the plan's agent rewrites the document from the conversation so far. A work session: 409 — its files are written by the agent's own tools, and there is no plan document to sync.

### POST `/api/conversations/{id}/doc` body

| field | schema | notes |
| :--- | :--- | :--- |
| `tier` | `string? nullish(60)` |  |

## `/api/conversations/{id}/files`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_files.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_files.rs)

> GET /api/conversations/{id}/files — the session's files, for the Work pane.
>
> WHAT THE PANE SHOWS. A work session's pane is the files the session has
> touched: every Talaria document and Google file a chip on one of its
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{kind, files}` | 200, 404 | — |

**GET** — The session's files, oldest first appearance first: every Talaria document and Google file the conversation's chips name (`touched`, with `lastTouchedAt` the newest message naming it), then the teammate's pins (`pinned`), then a legacy work session's living document (`living`). Artifacts the caller cannot read are left out; at most 100 files.

## `/api/conversations/{id}/members`

Source: [`api/crates/talaria-routes-boards/src/plans/plans_id_members.rs`](../../api/crates/talaria-routes-boards/src/plans/plans_id_members.rs)

> /api/plans/{id}/members.
> Multiplayer plan membership + presence.
>   GET    → { members, active, teams } — any member; active = user ids seen in the
>            last minute (Redis presence keys, 60s TTL).
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{members, active, teams}` | 200, 404 | — |
| POST | `session` | [body](#post-apiconversationsidmembers-body) | `{members}` | 200, 400, 403 | — |
| PUT | `session` | — | `{ok}` | 200, 404 | — |
| DELETE | `session` | [body](#delete-apiconversationsidmembers-body) | `{members}` | 200, 400, 403 | — |

### POST `/api/conversations/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `email` | `email` |  |

### DELETE `/api/conversations/{id}/members` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |

## `/api/conversations/{id}/messages/{msgId}/reactions`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_messages_msgid_reactions.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_messages_msgid_reactions.rs)

> /api/conversations/{id}/messages/{msgId}/reactions.
> POST { emoji } → toggle your reaction on a message in an agent DM (or any
> conversation you can read): add it if you haven't, remove it if you have.
> Answers { ok: true, reacted } — `reacted` is whether your reaction is now on.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiconversationsidmessagesmsgidreactions-body) | `{ok, reacted}` | 200, 400, 404 | — |

### POST `/api/conversations/{id}/messages/{msgId}/reactions` body

| field | schema | notes |
| :--- | :--- | :--- |
| `emoji` | `string(1, 16)` |  |

## `/api/conversations/{id}/read`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_read.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_read.rs)

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

## `/api/conversations/{id}/schedule`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_schedule.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_schedule.rs)

> /api/conversations/{id}/schedule.
> POST { name, schedule, prompt } → a standing cron on this thread's agent
> whose turns come back INTO this thread. The fleet's own cron door
> (/api/fleet/agents/{id}/crons) creates the same job and reports nowhere;
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `agent-manager` | [body](#post-apiconversationsidschedule-body) | `{ok, jobId}` | 200, 400, 403, 404 | audit |

**POST** — Put this conversation's agent on a timer: a cron job on the agent itself whose every run posts its answer back into this thread.

### POST `/api/conversations/{id}/schedule` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(1, 80)` | The same bounds the fleet door takes, because it is the same job. |
| `schedule` | `string trimmed(1, 120)` |  |
| `prompt` | `string trimmed(1, 5000)` | The agent's own cap: Hermes's /api/jobs refuses a longer prompt (talaria_agent_crons::MAX_PROMPT_CHARS). |

## `/api/conversations/{id}/schedules`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_schedules.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_schedules.rs)

> /api/conversations/{id}/schedules.
> GET → what this thread has put its agent on a timer for. Two halves: the
> rows Talaria wrote when the schedule was created (name, schedule, when,
> which job), and the live state only the agent's own scheduler knows — is it
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{schedules}` | 200, 404 | — |

**GET** — The cron jobs this conversation has scheduled on its agent, each with the live job state when the agent's container can be read.

## `/api/conversations/{id}/schedules/{jobId}`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_schedules_jobid.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_schedules_jobid.rs)

> /api/conversations/{id}/schedules/{jobId}.
> DELETE → take a thread's schedule away: the job leaves the agent's own
> scheduler and the row that made this thread its listener goes with it. The
> same two gates as creating one — seeing the thread (404 for one you cannot
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| DELETE | `session` + `agent-manager` | — | `{ok}` | 200, 400, 403, 404 | audit |

**DELETE** — Remove a schedule this conversation created: the job and its listener row.

## `/api/conversations/{id}/session`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_session.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_session.rs)

> GET /api/conversations/{id}/session — what the agent is still holding of
> this thread.
>
> A PINNED agent keeps the transcript itself: Talaria's conversation id IS the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{applies, compacted, since, sessionId, messageCount, partial, reachable}` | 200, 403 | — |

**GET** — What the agent itself still holds of this thread: how many messages it has compacted away and how far back the surviving transcript reaches. `applies` is false — with zeros and nulls — for an agent that is not pinned, a thread Hermes never held, or a container that is not answering; the compacted summaries themselves never cross the wire.

## `/api/conversations/{id}/steer`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_steer.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_steer.rs)

> /api/conversations/{id}/steer.
> POST { content } → say something to the agent WHILE it is replying, and have
> it account for it in the turn it is already in. Hermes hands a steer to a
> running agent mid-turn; without it, a correction typed over a streaming reply
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiconversationsidsteer-body) | `{steered, messageId}` | 200, 202, 400, 403, 409 | — |

**POST** — Say something to the agent mid-reply. `steered` means the running turn took the words; `queued` (202) means it did not and the row waits in history for the next turn. Nothing is streaming → 409.

### POST `/api/conversations/{id}/steer` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string(1, 20000)` | content: what to say to the agent mid-turn — a sentence of redirection, not a document, which is what the small ceiling says. |

## `/api/conversations/{id}/stop`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_stop.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_stop.rs)

> /api/conversations/{id}/stop.
> POST → interrupt the reply that is streaming in this thread right now. The
> stop button, server-side: Hermes addresses a running turn by its own run id.
> A session turn's run id is on its assistant row (stamped from
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | — | `{ok, hermes} or {ok, hermes, held}` | 200, 400, 403, 409, 502 | — |

**POST** — Interrupt the reply streaming in this thread, on either arm: a session turn by its run id, a completions turn by its completion id. A turn this instance is still opening (sent, but not yet named by the agent) is waited for, up to 10 s. The optional body `{ turnId }` names the page's own send: a stop that comes before that send has been read waits for it too, and is held for it past the wait. `hermes` says whether the agent's own run was interrupted, or whether hanging up was the whole stop (a completions turn a drain in another process reads). `held` says the agent had not opened the turn within the wait, and the stop is sent when it does. Nothing is streaming → 409.

## `/api/conversations/{id}/teams`

Source: [`api/crates/talaria-routes-boards/src/plans/plans_id_teams.rs`](../../api/crates/talaria-routes-boards/src/plans/plans_id_teams.rs)

> /api/plans/{id}/teams.
> POST { teamId } → grant a team (owner). DELETE { teamId } → revoke (owner).
> Keeps the plan doc's editor grants in step with the team principal.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiconversationsidteams-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | [body](#delete-apiconversationsidteams-body) | `{ok}` | 200, 400, 403 | — |

### POST `/api/conversations/{id}/teams` body

| field | schema | notes |
| :--- | :--- | :--- |
| `teamId` | `uuid` |  |

### DELETE `/api/conversations/{id}/teams` body

| field | schema | notes |
| :--- | :--- | :--- |
| `teamId` | `uuid` |  |

## `/api/conversations/{id}/todo`

Source: [`api/crates/talaria-routes-comms/src/comms/conversations_id_todo.rs`](../../api/crates/talaria-routes-comms/src/comms/conversations_id_todo.rs)

> GET /api/conversations/{id}/todo — the agent's plan for this thread.
>
> THE READ DOOR, gated on the conversation's own access rule (the owner, or a
> member of the plan / work session) — the same predicate every other
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{items, summary}` | 200, 403 | — |

**GET** — The steps the agent is working through on this conversation.

## `/api/dms`

Source: [`api/crates/talaria-routes-comms/src/comms/dms.rs`](../../api/crates/talaria-routes-comms/src/comms/dms.rs)

> /api/dms. Find-or-create a direct message (rides the channel machinery:
> same messages, SSE feed, and composer as everything else).
>
>   POST { userId }                       → the DM with that person.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apidms-body) | `…` | 200, 400, 403 | — |

### POST `/api/dms` body

| field | schema | notes |
| :--- | :--- | :--- |
| `userId` | `uuid` |  |
| `userIds` | `string[]?(36, 36)` |  |
| `agents` | `string[]?(1, 200)` |  |
| `name` | `string?(80)` |  |


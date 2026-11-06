# Producers

Who files which notification, and how: one row per thing with a count instead of one per
event, rows that resolve when their thing does, the people taking part in a conversation
told (each re-checked for access when the row is filed), and per-channel levels with a
timed mute. Every producer files through `add_notification_with` and the conventions in
[the contract](./contract.md) (I-24): `group_key::*`, `entity::*`, `NotifyActor`,
`Coalesce`, `resolve_entity`. The user-facing story stays in
[`../NOTIFICATIONS.md`](../NOTIFICATIONS.md).

## Feature flags

`threads::FEATURES` (served in `features` by `GET /api/notifications`, I-23):

| Flag | Means |
|---|---|
| `coalesce` | Rows carry a group key and coalesce: `count` rises and the open row is updated instead of a second row being filed. |
| `channel-levels` | The producers obey `channel_members.notify_level` and `muted_until` (the contract's `channel-levels-store` is the storage alone). |
| `participants` | Ticket comments and channel thread replies notify the people taking part. |

## The producers

| Producer | Kind | Group key | Coalesce | Entity | Actor | Who |
|---|---|---|---|---|---|---|
| A DM (`talaria-channel-replies`) | `dm` | `dm(channel)`, adopts a pre-contract row | Realert | `channel` | the sender | every other member, at their level |
| An @mention in a channel, room, ticket or plan | `mention` | none (each mention is its own row) | — | `channel`, `task` or `plan` | the sender (a person, or the agent by model id) | the members it names, minus level `none` |
| A channel thread reply | `thread-reply` | `channel_thread(root)` | Realert | `channel` | the replier | the root's author and earlier repliers, access re-checked |
| A message in a channel at level `all` | `channel-message` | `channel_activity(channel)` | Quiet | `channel` | the sender | members at `all`, top-level messages only |
| An agent's reply in your thread | `agent-reply` | `thread(conversation)`, adopts | Realert | `conversation` | the agent | audience members whose read cursor is behind the reply |
| Agent outreach | `agent-outreach` | `outreach(agent, UTC day)` | Realert | `conversation` | the agent | the person reached |
| A ticket's status move (`talaria-tasks`) | `task-status` | `task_status(task)`, 10-minute window | Quiet | `task` | the mover | watchers, assigned humans and the **creator**, access re-checked |
| A ticket comment | `task-comment` | `task_comments(task)` | Realert | `task` | the commenter | assigned humans, the creator, watchers and earlier commenters, minus the author and anyone the comment @mentions, access re-checked |
| Assignment, workchain turn and pause | `task-assigned`, `workchain_*` | none | — | `task` | the assigner, or the chain | as in wave 1 |
| An MCP server going down (`talaria-mcp`) | `mcp_server_down` | `mcp_down(server)` | Realert | `mcp_server` | the health pass | every admin |
| A Hermes agent image that failed its rollout, a release held for review (a Hermes internal Talaria relies on changed), an agent unhealthy on a known-good image or failing a catch-up, a newly good image withholding stored secrets (`talaria-hermes-channel`) | `hermes_update_failed` | `hermes("<digest>:failed")`; the rest `hermes("<topic>")`, once a day per topic | Realert | `hermes_image` | the stable channel | every admin |
| The agent image channel stuck: checks failing 3 days, a rollout open 7 days, the host failing a rollout three times in a row or short of disk for a download, an agent left behind three windows, its state unreadable | `hermes_channel_stuck` | `hermes("<topic>")`, once a day per topic | Realert | `hermes_image` | the stable channel | every admin |
| A workbench harness release held (Oh My Pi or the harness's bun would not install, start or pass the preflight on a workbench), a workbench that will not take the verified pair, the host failing the smoke test three times in a row (`talaria-workbench-updates`) | `workbench_harness_update_failed` | `workbench_harness("held:<tool>:<version>")`, `workbench_harness("volume:<department>:<bun>+<omp>")`, `workbench_harness("host")`, once a day per topic | Realert | `workbench_harness` | the workbench harness channel | every admin |
| An approval waiting (`talaria-approvals`), and its SLA nag, escalation and stall report (`talaria-digest`) | `approval_pending` | `approval(key)` | Realert | `approval` (the approval's stable key) | the approvals system | the authority (content) and the fact audience |
| Research ready, research and plan shares, KB comments, gaps, agent problems, the Google principal, the board-access outcome | as in wave 1 | none | — | `research_run`, `plan`, `kb_doc` or `task` where one exists | the person or agent who caused it | as in wave 1 |

A post with no text — an attachment or a ref chip on its own — notifies exactly as a line of
text would (the DM row, a thread reply, the channel's `all` row); the row's body reads
"Sent an attachment".

**Realert** moves `createdAt` later on each coalesce, so every client reads a re-alert (a
known id whose `createdAt` moved). **Quiet** updates the count and the content and keeps
`createdAt`: the first write alerts, the rest do not, and nothing is pushed or mailed for
them. A read row has left the open-group index, so the next write files a fresh row.

### Status bursts

The status row coalesces for ten minutes after its last change
(`talaria_tasks::status_window_secs`; `TALARIA_NOTIFY_STATUS_WINDOW_SECS` shortens it for
a live check). The first move's body is the ticket's title; later moves in the window say
`Moved 4 times — now in review`. The count in that line is the row's own: each move's
fan-out runs detached, so moves close together can land in any order and word the line
from a stale count; after its write, a fan-out whose count differs rewords the body, but
only while the row still holds its seq, so the last write keeps the last word. Past the
window, the old row keeps its content and loses its key, and a fresh row is filed. A person the same patch assigns is told by the
`task-assigned` row instead (one action, one row).

### Resolutions (I-16)

- **Approvals.** A decision reads every recipient's rows about the approval at once, and
  each of their devices hears the read (`readIds`): the board-access decision route
  (`PUT /api/boards/{id}/agent-requests`), and a ticket leaving review (moved out, rejected
  in review, or archived). The announce sweep is the floor for the decision routes that do
  not call it: a key it announced that the complete census no longer lists is resolved on
  the next pass (at most five minutes). The SLA stages file into the row the person already
  holds, so a nag re-alerts it (`count + 1`) rather than adding a second row.
- **MCP servers.** Recovery reads every admin's down row. A server that goes down again
  within an hour of recovering (`FLAP_WINDOW_SECS`) reopens that row through
  `reopen_notifications` and re-alerts it, so a flapping server is one row with a count.

## Participants and access (SE-9, C24)

Each recipient's read access is re-checked when the row is filed, not when they first took
part: `board_role` for a ticket's board, `channel_role` for a channel (a member row is
access; anyone else is asked). Someone removed from a board since they commented gets
nothing. Nobody gets two rows for one message: a mention wins over a thread reply, which
wins over the channel's activity row, and a ticket comment leaves out the people it
@mentions.

Every door a ticket comment can land through reaches the participants: the comment route,
a post in the ticket's room, the MCP comment tool, and an agent's streamed reply all end in
`talaria_tasks::room_comment_fanout`. A thread reply in a ticket room is a ticket comment
(`task-comment`), not a `thread-reply`.

## Channel levels and the timed mute (O-14)

| Level | Channel | DM |
|---|---|---|
| `all` | one `channel-message` row per channel, counting later messages quietly; @mentions and thread replies as usual | every message (the default) |
| `mentions` | @mentions and the threads you are in (the default) | only messages that @mention you |
| `none` | nothing, @mentions included | nothing |

A running mute (`muted_until > now()`) lands every row quietly: `alert: "feed"`, counted
in the bell, never toasted, bannered, pushed or mailed. The producers read every member's
level and mute in one query per post (`threads::channel_listeners`).

- `PUT /api/channels/{id}/notify` sets them on the caller's own member row (the contract's
  route); `GET /api/channels/{id}` serves them back as `notify: {level, mutedUntil,
  settable}`, with `mutedUntil` null once a mute has run out.
- **The menu** (`ui/src/components/chat/ChannelNotifyMenu.svelte`, beside the star in the
  conversation header): All / Mentions / Nothing; Mute for 1 hour, 8 hours, 24 hours, or
  until I turn it back on; and Unmute. The same component's `status` part puts "Muted until
  …" after the conversation's name and topic; in a tight header that line gives way first
  (down to its icon, the words in its tooltip and the bell's), never the name. A write puts
  the server's answer straight into the channel-detail query both parts read, then
  refetches, so nothing local outlives the server's state. The menu is hidden when the api
  serves no `notify` (an older server) and when `settable` is false. A refused write is
  shown in a dialog: a 403 as "You're no longer in this conversation…", a 409 in the
  server's own words.

### Team-roster members have no level (S10, an owner decision)

Someone who reaches a channel through `channel_teams` has no `channel_members` row, so
nothing can hold a level of their own: `PUT …/notify` answers 409 ("you're in this channel
through a team, and per-person notification settings for team members aren't available
yet"), `settable` is false, and the menu is hidden. They also get no `channel-message`
rows and, as in wave 1, no @mention rows from a channel post (the mention producer reads
member rows), though a thread reply still reaches them after the access re-check. Inserting
a member row for them would outlive their team membership, so giving them levels needs a
per-person table. That is the coordinator's decision, and nothing here invents the schema
for it.

## Cancel safety (C30, C34)

A handler future is dropped when its caller hangs up, at whatever await it had reached. A
direct-api agent with a short timeout hangs up after the write commits, and the notify
work still queued behind the write's tail never ran. Measured on the slot-9 stack with a
500 ms pause injected after the commit: 0 of 10 mention rows before, 10 of 10 after.

- `PUT /api/tasks/{id}` and `POST /api/channels/{id}/messages` (a person's door and an
  agent's) run the write and everything it owes people inside one spawned task the handler
  awaits. A spawned task is not the caller's to drop.
- `update_task` spawns its notices (the status move, the newly assigned, the workchain
  hand-off, a decided review) right after the ticket's UPDATE, before the activity-log
  writes, so every other caller gets the same.
- The board-access decision spawns its resolution before its next await (the task fetches
  its own redis handle).
- The test pause is `TALARIA_TEST_POST_COMMIT_MS` in the task route, honoured in debug
  builds only.

## Plan-doc mentions (SE-8)

`notify_plan_mentions` read `list_users` as (id, name, email) where it answers (id, email,
name), so every candidate carried their email as their name, and `@ada` matched nobody
whose email was not literally `ada`. It also asked for each person's teams in a loop: 45
reads of `team_members` for one message in a 45-person workspace. Both are fixed: the
mention reaches the person, and one message reads `team_members` once, whatever the head
count.

## Contract notes

- No new kind was needed: every kind filed here is already in `classes.rs`'s `KIND_CLASS`.
- The @mention token rules moved from `talaria-mentions` into `talaria-notify`'s
  `threads.rs` (`mention_tokens`, `user_mention_tokens`, `answers_to_tokens`), so a ticket's
  comment participants can leave out the people a comment names without a new crate edge
  (the lock file is the engine stream's).

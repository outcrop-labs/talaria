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

`boards::FEATURES`:

| Flag | Means |
|---|---|
| `board-prefs` | The ticket producers obey each person's setting for a board (below, "Each board"), and `GET`/`PUT`/`DELETE /api/boards/{id}/notify`, `GET /api/notifications/boards` and the `boards` default in `GET`/`PATCH /api/notifications` are served. |

## The producers

| Producer | Kind | Group key | Coalesce | Entity | Actor | Who |
|---|---|---|---|---|---|---|
| A DM (`talaria-channel-replies`) | `dm` | `dm(channel)`, adopts a pre-contract row | Realert | `channel` | the sender | every other member, at their level |
| An @mention in a channel, room, ticket or plan | `mention` | none (each mention is its own row) | — | `channel`, `task` or `plan` | the sender (a person, or the agent by model id) | the members it names, minus level `none`; a ticket's: each by their setting for the board (Off drops it; a deliberate watch keeps it) |
| A channel thread reply | `thread-reply` | `channel_thread(root)` | Realert | `channel` | the replier | the root's author and earlier repliers, access re-checked |
| A message in a channel at level `all` | `channel-message` | `channel_activity(channel)` | Quiet | `channel` | the sender | members at `all`, top-level messages only |
| An event on a ticket a person is not in, on a board they hear in full (`talaria-tasks`) | `board-activity` | `board_activity(board)` | Quiet | `board` | the person or agent who caused it | members whose setting for the board covers the event over scope `board` (All, or Custom), not in the ticket's own audience |
| An agent's reply in your thread | `agent-reply` | `thread(conversation)`, adopts | Realert | `conversation` | the agent | audience members whose read cursor is behind the reply |
| Agent outreach | `agent-outreach` | `outreach(agent, UTC day)` | Realert | `conversation` | the agent | the person reached |
| A ticket's status move (`talaria-tasks`) | `task-status` | `task_status(task)`, 10-minute window | Quiet | `task` | the mover | watchers, assigned humans and the **creator**, access re-checked, each by their setting for the board |
| A ticket comment | `task-comment` | `task_comments(task)` | Realert | `task` | the commenter | assigned humans, the creator, watchers and earlier commenters, minus the author and anyone the comment @mentions whose mention row lands, access re-checked, each by their setting for the board |
| Assignment, workchain turn and pause | `task-assigned`, `workchain_*` | none | — | `task` | the assigner, or the chain | as in wave 1; a ticket's assignment: each by their setting for the board (Off drops it; a deliberate watch keeps it); a workchain turn or pause never reads it |
| An MCP server going down (`talaria-mcp`) | `mcp_server_down` | `mcp_down(server)` | Realert | `mcp_server` | the health pass | every admin |
| A Hermes agent image that failed its rollout, a release held for review (a Hermes internal Talaria relies on changed), an agent unhealthy on a known-good image or failing a catch-up, a newly good image withholding stored secrets (`talaria-hermes-channel`) | `hermes_update_failed` | `hermes("<digest>:failed")`; the rest `hermes("<topic>")`, once a day per topic | Realert | `hermes_image` | the stable channel | every admin |
| The agent image channel stuck: checks failing 3 days, a rollout open 7 days, the host failing a rollout three times in a row or short of disk for a download, an agent left behind three windows, its state unreadable | `hermes_channel_stuck` | `hermes("<topic>")`, once a day per topic | Realert | `hermes_image` | the stable channel | every admin |
| A workbench harness release held (Oh My Pi or the harness's bun would not install, start or pass the preflight on a workbench), a workbench that will not take the verified pair, the host failing the smoke test three times in a row (`talaria-workbench-updates`) | `workbench_harness_update_failed` | `workbench_harness("held:<tool>:<version>")`, `workbench_harness("volume:<department>:<bun>+<omp>")`, `workbench_harness("host")`, once a day per topic | Realert | `workbench_harness` | the workbench harness channel | every admin |
| An approval waiting (`talaria-approvals`), and its SLA nag, escalation and stall report (`talaria-digest`) | `approval_pending` | `approval(key)` | Realert | `approval` (the approval's stable key) | the approvals system | the authority (content) and the fact audience |
| A GitHub permission an agent asked for (`request_git_permission`) approved, denied, or held by GitHub at last (`talaria-routes-workbench`'s permission requests) | `git_permission_decided` | none (each decision is its own row) | — | none | the admin who decided, or the system when GitHub settled it | the agent's managers; the row names the agent, the permission, the host and the outcome, never the agent's words or the note |
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

## Each board

A person chooses, board by board, what they hear about its tickets
(`talaria_notify::boards`). The ticket producers ask it at fan-out, once per event, the
place channel levels are asked: the status move (`update_task`), the review decision, the
board move, creation with its assignment, the comment fan-out, and the four ticket
@mention doors (the create and edit of a description, the comment route, a post in the
ticket's room) through `MentionOpts.ticket`. Nothing at send time reads it: the deferred
push and the mail batcher keep reading the class matrix, as they do for a channel level.
The one other reader is the daily board digest (below), once a day, for one person's own
boards.

**The setting** is a preset — `all`, `participating`, `mentions`, `off` — or `custom`:
chosen events over a scope. The events are `created` (a ticket appeared on the board,
created there or moved onto it), `assigned`, `status` (any column move but one into a done
column), `comment`, `mention`, `due` and `completed` (a move into one of the board's done
columns). The scope is `mine` (the tickets the person is in: each producer's own
audience — watchers, assigned people, the creator, earlier commenters) or `board` (every
ticket). Presets are stored by name and expanded when read: `all` is every event over the
board; `participating` is assigned, status, comment, mention and completed over mine;
`mentions` is assigned and mention; `off` is nothing. `created` needs scope `board`
(nobody is in a ticket before it exists). `due` is listed but refused until its producer
ships. The cadence is `instant` (as it happens) or `digest` (the daily board digest,
below).

**Where it is stored.** One row per person per board in `board_notify_prefs` — a table,
not a `board_members` column, because a team's members see its boards with no member row
of their own. A person's default for every board sits under the reserved `boards` key of
`users.notify_prefs` (`boards::BOARDS_KEY`), written by `PATCH /api/notifications`.
Effective = the board's row, else the default, else the built-in, `participating` — the
audience every ticket producer had before this existed. So nobody hears anything new, or
less, until they choose, and a new board inherits the default by having no row.

**The order**, written once in `boards::resolve`:

1. Needs-you rows never ask it: approvals (ticket review included), judge escalations,
   workchain turns and blocked agents are filed by their own producers. A board set to Off
   still gets its review sign-offs.
2. A deliberate watch on the ticket — the watcher added it themselves (`task_watchers.
   added_by` is them), or it predates `added_by` (null: the only Watch the product
   offered then was a person's own) — carries that ticket's status, completion, comment,
   mention and assignment rows, even on a board set to Off. A watch an editor added for
   someone is involvement, not a choice, and does not wake a board they set to Off.
   Pressing Watch on such a watch makes it theirs; an editor's add never takes one back.
3. The person's setting for the board, then their default, then the built-in.
4. Then everything else, unchanged: the class matrix, DND, working hours, the instance
   switches and the email frequency.

Off silences assignments and @mentions from that board, as a channel at Nothing silences
@mentions. Whatever points at the person (assigned, @mentioned) is never held back by
scope.

**The busy-board cap.** An event on a ticket the person is not in, on a board they hear in
full, files ONE `board-activity` row per board (`group_key::board_activity`,
`Coalesce::Quiet`, entity `board`, href `/boards/<id>`): the first event files it, the rest
count into it with the newest line, until it is read — a channel at All, for a board. The
tickets the person is in keep their own per-ticket rows. Its class, `board_activity`, is
Following and quiet by default (the `work_complete` channels): news about tickets you are
not in never rings louder than the status moves of the tickets you are (`work_complete`,
quiet). A person who wants the banner turns that matrix row on. A visit to the board reads
the row (the visit rule, EB-13), and the next event files a fresh one.

**One action, one row** still holds, and only when the louder row lands: a person a patch
assigns leaves that move's audience only if their `task-assigned` row was filed, and a
person a comment @mentions leaves the comment fan-out only if their mention row lands
(`boards::mention_listeners`, the same question the mention door asks). A new ticket's
"New ticket" board row skips whoever its `task-assigned` row reached and whoever its
description @mentions whose mention row lands (the door's roster — the board's direct
members — and the same question). A Custom set that keeps comments but drops @mentions hears the @mentioning
comment as a comment; a set that keeps status but drops assignments hears a patch that
assigns and moves them as one status row.

**The daily digest.** A board heard at cadence `digest` files nothing as things happen: the
resolver answers `Digest` and the producers drop it — except what points at the person
(an assignment, an @mention) and the news of a ticket they watch themselves, which still
come at once. Once a day the `board-digest` job (`talaria-digest` `boards.rs`) reads the
board's own records and files one row per board. It runs in the daily digest's slot: the
`digest_config` hour in the person's own zone, a 15-minute tick, a two-hour window that never
wraps. The row counts, over the stretch since the person's last digest (the first covers a
day, none more than two): **new** tickets (created on the board or moved onto it — only for a
setting over the whole board; for the person's own tickets an arrival counts as a move, as the
board move's producer tells them), tickets **moved** (not into a done column), tickets
**completed**, **comments**, and — once due-date reminders have a producer — tickets **due
soon** or **overdue**. Due dates count as thresholds crossed inside the window, the way the
reminders fire them: due soon is a ticket that came within a day of its due date and is not
due yet, overdue is one whose due date passed; neither counts a ticket that was already past
it when the window opened, so a standing overdue backlog is never news by itself, and the
first digest replays nothing older than its day. Each only when the setting wants that
event, over the tickets it covers (`mine`: watching, assigned, created, and for a comment
having commented before; or the whole board). A ticket that arrived on the board inside the
window counts from its arrival: its moves and comments on the board it came from are not this
board's, and the column line a board move writes beside its `moved` line is the arrival, not
a second move or a completion. Never counted: the person's own actions, a tracker sync's
writes (`talaria_tasks_types::MIRROR_ACTORS`), archived tickets, and the arrivals, moves,
completions and comments of a ticket they watch themselves. Only the boards the person's
board list shows today — a board they lost is dormant — and only counts and the board's
name. Read at the slot, not queued: a change of setting applies to the next digest, and the
counts are a rollup that can differ slightly from what instant rows would have said (a
ticket counts once per line however often it moved; a ticket that left the board inside the
window is no longer counted on it). The counts query reads `task_watchers` directly for the
person's own watch rows, on boards they already passed `list_boards` for.

The row: kind `board-digest`, class `board_activity`, `group_key::board_digest(board, day)` —
one row per board per local day, so a day the person never opened keeps its own counts (the
next window starts where this one ended; a row that a newer day overwrote would lose them for
good) — entity `board`, href `/boards/<id>`, body like `3 new · 8 moved · 2 completed · 11
comments`. It is filed **quiet** — a digest never rings, and its email is not the
missed-activity batcher's but one **"Your boards"** mail listing every board with something
to report, sent when the person has an address, their Board activity Email is on and their
email frequency is not Never, through `send_gated_mail` (the instance switch holds it). A
person with no address still gets the rows. Nothing to report, nothing sent.

The ledger, `board_digest_state` (one row per person), is written as the pass goes, each
deciding write a compare-and-set. The day is claimed before its rows are filed, and the
claim marks it **filing** (`filing_claim`, the claiming pass's clock); nothing mails a day
while it is filing. When every row has landed, the claiming pass clears the mark — only if the
claim is still its own — and then claims the mail before sending it (a mail that failed or
was held gives the claim back, and the next tick inside the window offers it again without
filing the rows twice). A pass that dies or fails mid-filing leaves its claim: a pass inside
ten minutes (the job's `max_run_ms`) leaves it to its owner, and the next tick takes it over
— compare-and-set on the claim it read — and files only the rows still missing for the
claimed window (a row already filed for that day is found by its key). A day still filing
when the window closes keeps what landed, and the next digest starts after it. A tick inside
the same window, or a pass that overlaps another, files and mails nothing twice. It is a
separate mail from the approvals digest, at the same hour. Feature `board-digest`.

**A board move** tells the destination's audience by their setting for the destination,
the people who hear the destination in full that a ticket arrived (`created`), the people
losing sight of it by their setting for the source, and the people who hear the source in
full that it left — never where to — unless a per-ticket row about the move already
reached them.

**Who caused it.** An agent's move is a move: an agent that takes a ticket out of review
on a person's proven ask (`talaria-tasks` `asked`) is the actor, and the ticket's people —
the asker among them, when they watch, are assigned or created it — hear it as a status
move by their setting, like any other. A tracker sync's write is not news: the ClickUp
sync (`talaria-board-sync`, [BOARD-SYNC.md](../BOARD-SYNC.md)) writes with `quiet` set,
so its creates, assignments and moves file no per-ticket row and no board-activity row,
whoever hears the board in full, and its imported comments run no comment fan-out. The
tracker it mirrors already told its people.

**Access and leaving.** The board-wide audience is `talaria_boards::board_audience`
(`board_role`'s two legs: a member row, or the owning team). Every recipient is re-checked
at fan-out, as before, so a setting is dormant while its person cannot see the board; it is
kept when they leave and applies again when they rejoin (the dormant-watcher rule). It goes
when the person or the board is deleted. A team's members can set a board; they still get
no ticket @mention rows (the mention doors read `board_members`, as before this existed),
so a comment or a new ticket that @mentions one does not count as having told them: they
hear it as the comment, or on their board row, by their setting — never nothing.

**Humans only.** Rows go to people; the routes are session-only; agents are only ever
actors (excluded as before). There is no MCP tool.

**Confined.** A person's setting must change only what that person receives. The module is
never glob-re-exported, and `scripts/check-invariants.mjs`
(`board-notify-setting-outside-the-fan-outs`) allows `talaria_notify::boards`,
`board_notify_prefs` and `BOARDS_KEY` only in the per-recipient fan-outs and the caller's
own routes — no broadcast (a board event to apps, a channel or room post, a realtime frame)
can read one. It reads every spelling of the module path: the full path, a braced import,
an alias or glob of the crate, and inside talaria-notify `crate::`, `super::` and a bare
`boards::` (the crate root's `boards::FEATURES` excepted), so the send-time readers there
cannot reach it either. A future board-to-channel feed is board configuration, never a person's
setting.

**The routes.** `GET /api/boards/{id}/notify` → `{effective, source, stored, default,
registry}` (any board role; anyone else 403); `PUT` `{preset, events?, scope?, cadence?}`
replaces the caller's own row; `DELETE` goes back to the default. `GET
/api/notifications/boards` → `{default, boards: [{id, name, role, effective, source}]}` for
the boards the caller can see. `GET`/`PATCH /api/notifications` carry `boards` (the
default; `null` clears it). The registry (event, preset and cadence labels) is served,
never a client-side list.

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

- No new kind was needed for the 2026-10 producers: every kind filed here was already in
  `classes.rs`'s `KIND_CLASS`. The per-board settings added `board-activity` and the daily
  board digest's `board-digest`, both in a new Following class `board_activity` (not in
  `ORIGINAL_CLASSES`, so the C27 digest freeze holds).
- The @mention token rules moved from `talaria-mentions` into `talaria-notify`'s
  `threads.rs` (`mention_tokens`, `user_mention_tokens`, `answers_to_tokens`), so a ticket's
  comment participants can leave out the people a comment names without a new crate edge
  (the lock file is the engine stream's).

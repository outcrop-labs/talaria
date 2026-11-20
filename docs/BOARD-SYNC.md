# Board sync from ClickUp

A board can sync from one ClickUp list: one way, ClickUp → Talaria. Each ClickUp task
becomes a ticket on the board. The sync carries over status, assignees, priority, dates,
tags, the description, attachments and comments. After the first import the sync keeps
running, on a schedule and whenever someone presses **Sync now**.

This page covers the engine and its HTTP API. People reach it through Board settings →
**sync**, the board's **ClickUp ·** chip and a synced ticket's **Re-sync from ClickUp**; the
member guide is [`docs/user/boards-clickup-sync.md`](./user/boards-clickup-sync.md). The
code is in `api/crates/talaria-board-sync` (the engine) and
`api/crates/talaria-clickup` (the ClickUp v2 client). The generated route reference is in
[`docs/api/boards.md`](./api/boards.md) and [`docs/api/tasks.md`](./api/tasks.md).

## Who can do what

| Action | Who |
|---|---|
| Read the status (`GET /api/boards/{id}/sync`) | any board member |
| Browse ClickUp, preview, connect, change the mapping or cadence, pause and resume, replace the credential, disconnect | a board owner |
| Sync now, re-sync one ticket | a board owner or editor, while the sync is not [paused](#pausing) |
| See or clear the stored credential | an admin, in Admin → Secrets |

## Connecting a list

1. **Make a token.** In ClickUp, open Settings → Apps and generate a personal API token
   (`pk_…`). The sync reads ClickUp *as that member*, so it sees only what that member can
   see. Use a dedicated ClickUp member for integrations if you can. A personal token stops
   working when its member is deactivated in ClickUp, and a member who leaves takes their
   token with them.
2. **Pick the list.** Call `POST /api/boards/{id}/sync/browse` with `{token}` to get the
   workspaces. Call it again with `teamId`, then `spaceId`, then `folderId` to walk down
   one level at a time.
3. **Preview.** `POST /api/boards/{id}/sync/preview {token, listId}` writes nothing. It
   answers with:
   - the list's statuses, each with a suggested column;
   - the columns on offer, and which of them start agent work;
   - the first tasks as they would land;
   - who would be assigned and who would not, and why;
   - the labels the import would create;
   - oversized files in the sample;
   - an estimate of the first import at the token's rate limit.

   A suggestion is either an exact, case-insensitive match of a column's label or key, or
   this board's last mapping of the same list. Nothing else is guessed.
4. **Connect.** `POST /api/boards/{id}/sync` takes
   `{token, listId, statusMap, options?, intervalMinutes?}` and does four things:
   - checks the token and the list;
   - validates the mapping;
   - seals the token;
   - starts the first sync.

   The answer is `{sync, runId}`. It never contains the token: the status shows only its
   last four characters and the ClickUp account it acts as.

`options`:

- `includeClosed` (default false): make tickets for tasks that are already closed in
  ClickUp. A ticket that exists always follows its task when the task closes.
- `comments` (default true) and `attachments` (default true).
- `notifyApps` (default false): tell apps about the sync's writes — `ticket.created` for
  an imported ticket and `ticket.moved` for a move the sync makes. It is off by default
  because a first import of a long list would otherwise run such an app once per ticket,
  and once more per closed task (made in the intake, then moved). A mapping change that
  re-places tickets would too. Turn it on after the first import if an app should follow
  ClickUp's changes.

`intervalMinutes` is `null` (manual only), `15`, `60` (the default) or `1440`.

## The status mapping

The mapping is an explicit table from ClickUp status name to board column, saved on the
connection and changed with `PUT /api/boards/{id}/sync {statusMap}`.

- A target can be any board column, Blocked included. Done columns are allowed: a task
  closed in ClickUp moves its ticket to the done column the owner chose. The activity
  line says "moved to done (closed in ClickUp)". The sync never stamps a sign-off;
  `approved_at` stays empty.
- Agent-start (pickup) columns are allowed too. The move guards below make them safe, and
  the preview marks them.
- A status with no mapping lands in the board's sync intake, and the ticket gets a
  problem. That covers a status added in ClickUp after you connected, and a mapping that
  points at a column deleted since. The sync intake is the first open column that is not
  an agent pickup column. When the board has no such column, the task is refused with a
  problem instead.
- A ticket cannot start closed, so a closed task is made in the intake and then moved.
  That is two writes.
- Changing the mapping re-places the tickets the old mapping placed. A ticket a person has
  moved since stays where they put it. A run applies the change without asking ClickUp
  anything. A live run applies it in its last pass; otherwise a remap run starts.

### What the sync will not move

The sync never takes these decisions away from a person or an agent, whatever the
mapping says:

- It never moves a ticket out of review. Leaving review would resolve the reviewer's
  approval.
- It never moves a ticket out of Blocked. A person unblocks a ticket.
- It never moves a ticket out of a working column while an agent's work session is live
  on it.
- It never moves a ticket into a pickup column while the ticket has agent assignees. That
  move would start their work, and the sync never starts an agent on the ticket it moves.
- It never adds or removes agent assignees.

When it holds a move, the ticket gets a `status-held` problem that says why. The next
change in ClickUp asks again.

### Workchains: only a close hands a step on

Only a move into a **done** column hands a workchain step on. It is a done move like a
person's: the chain hands on, and the next step's agents start as if a person had closed
the ticket. The board owner mapped that status to done, and a person closed the task in
ClickUp. On a board whose workchains should wait for a sign-off in Talaria, map closed
statuses to another column.

No other move counts as a close:

- A move into a review column (a "ready for review" status), Blocked, or a working or
  pickup column leaves the step live. Its successors stay blocked, nothing is offered to
  their agents, nobody is told it is their turn, and the chain does not pause.
- A task closed in ClickUp while its ticket is in review or Blocked is held, like any
  move out of them. A person moves the ticket on, and their done hands the step on.
- The sync cannot move a ticket to failed or cancelled, the columns that pause a chain:
  they are not mapping targets.

The rule lives in the workchain engine (`talaria_workchains::advance_workchains` hands
on only for the board's done columns). The live test
`board_sync_live::only_a_sync_move_into_a_done_column_hands_a_workchain_step_on`
walks a synced step through review, a held close, Blocked, a held unblock and a working
column, and then a close, against real work dispatch.

## What carries over

| ClickUp | Talaria |
|---|---|
| name | title, cut at 300 characters |
| markdown description (else plain text) | description, cut at 20,000 characters with a closing "(truncated — full text in ClickUp: …)" line |
| status | the column from the mapping |
| priority urgent / high / normal / low / none | urgent / high / medium / low / medium |
| due and start dates | due and start dates |
| tags | labels: the first 20, at 40 characters each, created on the board when missing |
| assignees | board members matched by email (below) |
| sub-tasks | sub-tickets under the top-level parent. Talaria nests one level; ClickUp nests deeper |
| attachments up to 25 MB | uploads on the ticket |
| comments and threaded replies | messages in the ticket's discussion |

Not carried over: time tracking, custom fields, checklists, points, comment attachments,
and ClickUp docs and goals.

**Assignees.** Each ClickUp assignee's email is matched against Talaria accounts, ignoring
case:

- A member of the board is assigned.
- A Talaria user who is not on the board is left unassigned, with a problem: add them in
  People, then sync again.
- An unknown email is left unassigned, with a problem.
- So is a member whose email ClickUp does not share, such as a guest.

These problems clear on the next sync that resolves them.

**Comments** are attributed to the original author as a label, "Dana Reyes (ClickUp)".
They are stored with `author_type` `imported` and their original time. The label never
contains an email and is never matched against Talaria accounts, so an imported comment
cannot land as a teammate's. Comments land oldest first, and a reply threads under its
comment. An edit made in ClickUp to a comment is mirrored; an edit to a reply is not,
because replies are read once, when they arrive. A comment deleted in ClickUp stays. The
sync reads at most the latest 1,000 comments of a task.

**Attachments** over 25 MB are never downloaded. They are listed as problems with their
ClickUp link. A fetch that fails is tried again on the next two runs and then listed.

## The conflict rule

The rule is field by field. The sync remembers the last value it wrote from ClickUp for
each field.

- If ClickUp's value is unchanged since then, Talaria's value stands, so a person's edit
  survives.
- If ClickUp changed the field, ClickUp's value is written.
- Labels and human assignees merge as sets: one added in Talaria stays, and ClickUp's own
  additions and removals land.

`POST /api/tasks/{id}/resync` (**Re-sync from ClickUp**) forces one ticket to match
ClickUp:

- title, description, priority and dates;
- the status, through the move guards;
- ClickUp's labels and human assignees replace the ticket's.

Agent assignees are left alone.

## Archived, deleted and moved tasks

The sync marks these tickets and never deletes one:

- **Archived in ClickUp**: the ticket shows it. A task unarchived in ClickUp turns active
  again.
- **Deleted, moved to another list, or no longer visible to the token**: these are found
  by the sweep (below), and the ticket shows which.
- **A ticket deleted in Talaria** is never made again.
- **A ticket moved to another Talaria board** keeps its link, and the sync leaves it alone.

**One task, one board.** A ClickUp task syncs to at most one board per instance. A second
board connected to the same list skips those tasks and reports them (`other-board`). To
move a list to another board:

1. Disconnect the first board. While its connection is live, it keeps its tasks.
2. On the first board, delete the tickets, or move them to the new board.
3. Connect the new board, or press **Sync now** on it. A task whose ticket was deleted is
   imported afresh, and a ticket moved to the new board is taken over where it sits, with
   its history. A task whose ticket still sits on another board stays skipped.

## Cadence, and never silently stale

The `board-sync` scheduler job runs every minute. It is a required job, so a missing
registration refuses to arm the schedule. Each tick:

- starts a run for every active connection that is due (a paused one never is);
- moves the connection's next sync out by one interval, whatever the outcome;
- skips archived boards;
- prunes finished sync runs older than 14 days, keeping each connection's last run.

The status (`GET /api/boards/{id}/sync`) answers:

- last and next sync, and the live run's phase;
- the latest run's result or error;
- `overdue`: an active, scheduled connection whose next sync is more than five minutes
  late with nothing running;
- `schedulerArmed`: false when the instance runs with `TALARIA_SCHEDULER=off`. Only Sync
  now syncs then, and it still works;
- a warning when the member who connected is no longer on the board, since their token
  still drives the sync;
- the current problems of this connection: the first 50, and the total. A remap or a
  one-ticket re-sync reads nothing from the list, so it changes neither the last sync time
  nor the run-level problems the last full run left.

`POST /api/boards/{id}/sync/runs` (**Sync now**) answers the run already going, if there
is one.

## Pausing

A pause **freezes** the sync. An owner pauses to fix the mapping, or to take a last
import before archiving the board, and nothing may undo that while it holds. While the
sync is paused, nothing reads ClickUp:

- the scheduler never claims it (it claims only active connections);
- **Sync now** (`POST …/sync/runs`) and **Re-sync from ClickUp**
  (`POST /api/tasks/{id}/resync`) answer `409` "the ClickUp sync is paused — a board
  owner resumes it in Board settings → Sync", for owners and editors alike;
- pressing Pause cancels the live run that reads ClickUp, so its in-flight step stops
  at its next await;
- a run that raced all of that, started a moment before on any instance, stops at its
  next step with "the sync is paused". It stops without its finish pass, so the
  watermark stays where the last finished run left it, and the run after Resume reads
  from there.

`PUT /api/boards/{id}/sync {paused: true}` pauses and `{paused: false}` resumes. Both
are a board owner's, the same people who configure the sync. The pause records who
pressed it and when (`board_syncs.paused_by`, `paused_at`). The status answers them as
`pausedBy` (`{id, name, email}`) and `pausedAt`, so the board shows "Paused by Dana Reyes ·
5m ago" to every member. Resume clears both, turns the connection active, and sets the
next sync one interval out. A `PUT` that names no `paused` never changes the state: the
write works it out from the row it changes, not from an earlier read, so a mapping or
schedule edit saved just as someone presses Pause leaves the pause in place.

What still works while paused:

- **a mapping change.** It applies through a remap run, which reads nothing from ClickUp
  ("fixing the mapping" is a reason to pause), so the owner sees the fix at once;
- **the schedule and the options**, which take effect on Resume. A paused sync has no next
  sync;
- **Replace credential.** The sync stays paused, with no next sync and no run. A sync whose
  token died while it was paused returns to paused, not active, when the token is
  replaced;
- **Disconnect**, from any state.

## The credential

The token is sealed with the secretbox in `board_syncs.credential_sealed`. That column is
in `CIPHER_TARGETS`, so key rotation re-seals it. Admin → Secrets lists it as
`board-sync:<id>`. The token belongs to the board, not to the person who pasted it.

Every run starts with `GET /user`. When ClickUp refuses the token, the run stops before
reading any task, so a dead token is never mistaken for "every task was deleted". Then:

- the connection waits in `needs_credential`;
- its status says so to every member;
- the board's owners and the instance's admins get one grouped notification (kind
  `board-sync`). A second failed run does not file another.

`PUT /api/boards/{id}/sync/credential {token}` replaces the token and keeps the mapping,
the links and the history. The connection turns active and a sync starts, unless it was
paused when the token died: then it returns to paused (see [Pausing](#pausing)).
Clearing the credential in Admin → Secrets also puts the connection in
`needs_credential`.

When the token works but the list is gone, or its member can no longer see the list, the
run stops with a `list-gone` problem and the same people get the same grouped
notification, once. The connection stays active and keeps trying on its schedule, so it
resumes on its own when the list is visible again. Until then each run stops quietly; the
notification comes again only after a run has finished in between.

A 401 on any other read (one task) means that task is out of the token's reach, not that
the token is dead.

## Rate limits

ClickUp limits each token per minute:

| ClickUp plan | Requests per minute |
|---|---|
| Free, Unlimited, Business | 100 |
| Business Plus | 1,000 |
| Enterprise | 10,000 |

A first import needs about two requests per task, plus one per 100 tasks for the list
pages. At 100 a minute, 2,000 tasks take about 40 minutes. The preview's estimate uses the
limit ClickUp reported to it. When the budget runs low, the run waits for ClickUp's reset
and says so ("Waiting for ClickUp's rate limit — resumes in 40s"), then carries on from
where it stopped.

## Side effects

The sync's writes go through the task engine as `clickup-sync`, so board membership,
activity lines, realtime, search indexing and the agent rules all apply. Its writes are
quiet:

- Nobody is notified about imported tickets, assignments or moves, including people who
  set the board to hear every ticket: a sync write raises no board-activity row. ClickUp
  already told its own people.
- The board gets one live event per batch, not one per ticket.
- Workchains still advance when a ticket lands in a done column, and the next step's
  agents start. No other sync move advances one (see
  [Workchains: only a close hands a step on](#workchains-only-a-close-hands-a-step-on)).
- Apps hear that a ticket was created or moved only with `notifyApps` on.

Imported comments do not notify anyone and raise no unread badge. Ticket discussions are
outside every unread count.

## Disconnecting

`DELETE /api/boards/{id}/sync` stops future syncs, cancels a run in flight and clears the
token. Every imported ticket, link and imported comment stays, and the tickets are
editable and owned by Talaria. Afterwards the status answers with the list the board was
disconnected from. Connecting the same list to the same board again re-attaches every
ticket by its ClickUp id, with no duplicates, and the preview pre-fills the old mapping.
Connecting a different list leaves the first list's tickets alone: they are not that
list's, so its sweep, its problems and its card chips leave them out.

## How a run works

A `clickup-sync` run is a durable run (talaria-runs-define) owned by the board: it has no
owner, its subject is the board, and every member may watch it. Only one runs per board
at a time, whoever started it (`runs_one_live_board_sync`). It works in passes:

| Pass | What it does |
|---|---|
| probe | `GET /user` and `GET /list/{id}` |
| incremental | tasks updated since the watermark, minus two minutes of slack, oldest first, a page at a time |
| archived | the same, for archived tasks |
| sweep | every page. It marks what it saw, and makes or updates any task the incremental walk slipped past. It runs on the next run after the first sync, on every Sync now, and at most daily on the schedule |
| missing | linked tasks the sweep did not see: one `GET /task` each, to tell deleted, moved and hidden apart. When more than a fifth of the linked tasks (and more than ten) went missing at once, it marks none and says so |
| attachments | pending files, one at a time |
| parents | sub-tasks whose parent had no ticket yet when they were made |
| remap | a mapping change |
| finish | moves the watermark to the run's start (only a finished run moves it), and records the run's problems |

A step stops after 60 seconds, or when ClickUp's remaining budget falls under eight
requests, and always saves its position before it waits. Every write is idempotent:

- tickets are keyed by `tasks.external_key` (`clickup:<task id>`);
- comments by `channel_messages.import_key`;
- files by the item's record.

The sync records a field as written only after the write lands, so a run that is resumed
after a crash finishes what the crash interrupted.

## Not verified against a real ClickUp workspace

The client was built from ClickUp's API documentation. It is tested against fixtures and
a loopback stand-in, and has not yet run against a real workspace. These behaviours still
need checking against one:

- whether a new comment moves a task's `date_updated`. If it does not, a new comment
  arrives with the next change to its task, or with a Re-sync from ClickUp;
- whether attachment links need the token. The sync fetches them without it;
- the exact error code for a deleted task. The sweep guard protects against misreading
  it.

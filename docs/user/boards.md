# Boards

Boards are where your team's tracked work lives: a board holds **tickets** that people and
agents move through a workflow of columns you define. Agents can work tickets but never
close them — finished agent work lands in a review column for a person to sign off. Every
board belongs to a person or a team, and who can edit what is set per board.

Open: **Work → Boards** in the sidebar. Admins can hide Boards from individual people
(Admin → People).

## To…

| Do this | How |
| :--- | :--- |
| Create a board | **New board** (sidebar footer) → name it → pick an owner (Personal or one of your teams) → **Create board** |
| Find a ticket | Type in **Search** (matches title, reference, description, labels), or use the filter pills: **Status · Assignee · Priority · Label · Due** |
| Save a search | Set the filters you want → **+ Save view** → name it. The view becomes a tab everyone on the board can use |
| Change columns | Board settings (gear) → **statuses** — add, rename, reorder, recolor, or delete columns |
| Invite someone | Board settings → **people** → pick a person → choose **Owner**, **Editor**, or **Viewer**. An owner can remove another owner; the last owner stays |
| Allow agents on a board | Board settings → **agents** → pick the agents (or tick **Allow all agents**) → **Save** |
| Move a ticket to another board | Ticket → **Move to board…** (or right-click → **Move to board…**) → pick the board, and a column if you want one → the dialog lists what changes → **Move**. You need to be an editor or owner on both boards |
| Archive what's done | Board: settings → **general** → Danger zone → **Archive**. Ticket: right-click → **Archive** |
| See archived tickets | Flip the **archived** chip in the toolbar — archived tickets otherwise leave the board |
| Review agent work | Open the ticket in **Quality review** → read the result → **Approve** or **Request changes** |
| Work on my queue | **Work → Inbox** → the Boards tab shows **To triage**, **To review**, **Blocked** |
| Sync a board from ClickUp | Board settings → **sync** → paste a ClickUp token → pick the list → check the preview → **Connect and start syncing**. [The full guide](./boards-clickup-sync.md) |

## The three views

Every board renders three ways — toggle with the icons top-left (the choice is remembered
per board, per browser):

| View | For | Notes |
| :--- | :--- | :--- |
| **Board** (kanban) | Moving work through columns | Drag cards between columns; quick-add with **Card title** in any column; column headers total the estimated hours |
| **List** | Working many tickets | Group by status, priority, assignee, or label; choose and reorder columns; checkbox-select for bulk **Move to / Priority / Assign to me / Archive** |
| **Gantt** | Deadlines | Drag bars to reschedule; zoom in/out; a **Today** marker |

Filters, search, and the active view all live in the URL — copy it to hand someone exactly
what you're looking at.

## Workflow columns

Columns are yours to define; their **category** is what carries meaning:

| Category | Meaning |
| :--- | :--- |
| **intake** | Where new tickets land. Every board needs one |
| **active** | Work in progress |
| **review** | The human sign-off queue. Required while agents are on the board — an agent may not sign off its own work |
| **done** | Closes the ticket. Every board needs one |

Tick **agent start** on a column to let agents pick up work from it — assignment there counts
as approval to start. **Blocked** is a system column: always present, not renamable, and only
a person can unblock a ticket from it.

If the columns can't support the agents you allowed, a warning panel on the statuses tab
tells you exactly what's stuck and how to fix it.

## Ticket details

Open a ticket (click a card). The description stays put at the top. Under it, tabs:
**Discussion** (the ticket's room — `@`mentions notify people), **Attachments** (files,
knowledge docs, and artifacts), and **Activity**. A ticket in a workchain also has a
**Workchain** tab; one with an agent result has a **Result** tab. The properties rail carries the rest:
status, priority, color, assignees, effort, estimate, time spent, tokens (what the agent's
work cost), dates, parent/sub-tasks, dependencies (**Blocked by / Blocks**), labels, and
watchers (**Watch** to follow a ticket).

Right-click any ticket for the fast lane: move, priority, color, due date, assign to me,
archive.

## Notifications

Each board has a bell in its header, beside **Board settings**: what you hear about this
board's tickets. Its picture is the setting — a ringing bell for **All activity**, a bell
for **Participating** (the tickets you're in: assigned to you, watched, created by you,
commented on), **@** for **Mentions & assignments**, a crossed-out bell for **Off** — and
hovering it says so ("Board notifications: participating (your default)"). Click it to
change it in one go. **Off** silences the board's assignments and @mentions too;
approvals still reach you, and so does a ticket you watch yourself.

The same menu turns on the **Daily digest** for the board: nothing as it happens, and once
a day, at the daily digest's hour, one quiet notification counting what the board did —
assignments, @mentions and tickets you watch yourself still come at once (a small clock on
the bell says the board is on a digest).

**Notification settings** in that menu opens **Board settings → Notifications**, for the
rest: **Custom** (pick the events — new ticket, assigned to me, status change, comment or
reply, mention, due soon or overdue, ticket completed — and whether they cover the tickets
you're in or every ticket on the board), **When** (instant or the daily digest), and the
board's own **Alerts** (the pop-up banner and the push) and **Email** (the missed-activity
email), each *From settings*, *On* or *Off*. Due-date reminders are off until you choose
them: All activity or a Custom set with "Due soon or overdue". Your inbox
keeps every notification whatever you choose. **Use my default** puts the board back on
your default. Anyone on the board has the bell and this tab, viewers included, and it only
ever changes what reaches *you*.

Your default for every board, and every board's setting at a glance, are in **Settings →
Notifications → Boards**. A board you haven't set follows the default; a new board does too.

A ticket you watch because someone else added you says who ("added by …"), with **Watch it
myself**: only a watch you made yourself reaches you on a board you've turned off.

**Moving a ticket to another board.** The ticket gets the new board's next reference
(`PLAT-118` becomes `OPS-7`); the old reference and old links still open it. Comments,
history, attachments and dependencies come along. It lands in the column of the same name,
else the same kind of column, else the new board's intake — never in a done column unless it
was already closed: a move is not a sign-off. Labels the new board does not have are dropped
(not created), and people or agents who are not on the new board are unassigned — the dialog
names each before you confirm, and the ticket's activity records it. A ticket an agent is
working on right now, a workchain step, or a ticket with sub-tasks can't be moved until that
is cleared. Agents can move tickets too (`move_ticket`), only between boards they are on,
never into a done column, never a ticket that is blocked, never one in review unless you ask
them to (below) and you can edit both boards, and never in a way that unassigns a person. Anyone who could see the ticket before the move and can't after it
(its assignees, watchers and creator) gets a short notice that it moved to a board they
can't see — without the board's name. An old link opens the ticket for anyone who can see
its new board. The ticket's token spend counts toward the board it is on now.

**Muse** (in the ticket detail) edits fields from plain language — type
`urgent, due friday` — or select text while editing and ask for a rewrite; review the chips,
then **Apply**.

## Working with agents

| Agents can | Agents can't |
| :--- | :--- |
| Be assigned to tickets (by people) | Assign tickets — agent-created tickets land in **Inbox** for a person to assign |
| Move work **forward**: into progress, into blocked, into review | Take a ticket out of blocked, or close one — a person signs off |
| Send a ticket back **out of review when you ask them to**: to a revisions column, Blocked, or back to work | Take a ticket out of review on their own, or close it even when you ask |
| Comment (with `@`mentions), even on closed tickets | Work archived tickets |
| Report a **Result** (outcome, resolution, error), time spent, and token cost | Edit archived boards |

**Asking an agent to send a ticket back.** Say it to the agent — in your chat with it, or in
the ticket's discussion — for example "ABC-12 needs revisions, move it back to In progress."
The agent makes the move and the ticket's activity says who asked: *"moved to in_progress
(asked by you in chat)"*. Talaria checks the ask before it lets the move through:

- it is the message the agent is answering right now, and you wrote it — an option you
  clicked on the agent's own question does not count, because those are the agent's words;
- the agent is answering you alone: if someone else wrote in the same turn, or the agent is
  writing two replies in one channel at once, Talaria cannot tell who asked;
- you can edit the board (both boards, for a move to another board), so an agent never does
  for you what you could not do yourself;
- your message names the ticket — its reference (`ABC-12`, an old one too) or a link to it —
  unless you said it in the ticket's own discussion. On a board without ticket numbers,
  paste the link;
- one message moves a ticket once.

Done, failed and cancelled stay yours even when you ask. Moving a ticket back into an agent
start column (In progress, on the default columns) starts work on it, exactly as dragging it
there does, and clears an earlier sign-off. When the check fails, the agent says why and
leaves an **Approve** card for the move in its reply: anyone who can edit the board can click
it, and the move is then made as that person. In a channel, the agent's reply has to start
within 15 minutes of the move it makes; a longer reply cannot prove the ask, and the card is
the way through.

Two approvals can wait for you inside a ticket:

- **Quality review** — **Approve** completes the ticket; **Request changes** sends it back to
  work. Optionally a judge model posts its verdict first (board settings → general →
  **QA judge**: advisory, or enforcing, which bounces failing work back to the agent up to
  three times before a human sees it).
- **Approve plan** — when an agent's coding work needs the go-ahead, the ticket shows what it
  plans to build; PRs it opens later appear here too (**View PR**).

The full agent side of this — mentions, chats, credential handoff, the review walk:
[Working with agents](./working-with-agents.md).

## Syncing from ClickUp

A board can follow one ClickUp list: each task becomes a ticket, and the board keeps
following the list on a schedule and on **Sync now**. It is one way — nothing is written back
to ClickUp. A board owner connects it from Board settings → **sync**; everyone on the board
sees how it is doing on the **ClickUp ·** chip beside the gear, with the problems it could not
resolve and how to fix each. A synced ticket links to its ClickUp task, and an editor can
**Re-sync from ClickUp** from the ticket. Disconnecting stops future syncs and leaves every
imported ticket on the board. Setup, the mapping, the conflict rule and every problem's fix:
[Syncing a board from ClickUp](./boards-clickup-sync.md).

## Who can do what

| | Viewer | Editor | Owner |
| :--- | :-: | :-: | :-: |
| See board, comment on open tickets | ✓ | ✓ | ✓ |
| Edit tickets, drag, bulk actions, save views | | ✓ | ✓ |
| Board settings (columns, labels, people, agents) | | ✓ | ✓ |
| Archive / restore | | ✓ | ✓ |
| Move a ticket to another board (editor or owner on both) | | ✓ | ✓ |
| Delete board, move it between teams | | | ✓ |
| See the ClickUp sync chip and its problems | ✓ | ✓ | ✓ |
| ClickUp: Sync now, Re-sync a ticket | | ✓ | ✓ |
| ClickUp: connect, change the mapping or schedule, replace the token, disconnect | | | ✓ |
| Add or remove an owner | | | ✓ |

Sharing a board requires the person to have signed in once already. Team boards: team members
are editors, team owners own the board.

One naming note: what the UI calls a ticket, the API calls a task — same thing, two words.

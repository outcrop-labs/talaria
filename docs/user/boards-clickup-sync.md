# Syncing a board from ClickUp

A board can follow one ClickUp list. Each ClickUp task becomes a ticket on the board, and
the board keeps following the list: on a schedule, and whenever someone presses **Sync now**.
The sync is one way, ClickUp → Talaria. Nothing is ever written back to ClickUp.

A board owner sets it up from **Board settings → sync**. No developer is needed, and there is
no configuration file to edit. The engine behind it is described in
[`docs/BOARD-SYNC.md`](../BOARD-SYNC.md).

## To…

| Do this | How |
| :--- | :--- |
| Connect a list | Board settings (gear) → **sync** → paste a token → pick the list → check the preview → **Connect and start syncing** |
| See how the sync is doing | The **ClickUp ·** chip beside the gear, on the board. Everyone on the board sees it |
| Sync right now | The chip → **Sync now**, or Board settings → sync → **Sync now** (owners and editors) |
| Fix a problem | The chip or the sync tab lists each one with its fix and a link to the ticket |
| Change where a status lands | Board settings → sync → **Status mapping** → pick the column → **Save mapping** |
| Pull one ticket from ClickUp again | Open the ticket → **Re-sync from ClickUp** (the circular-arrows button in its header) |
| Replace the token | Board settings → sync → **Replace credential** |
| Freeze the sync for a while | Board settings → sync → **Pause**. Nothing syncs until an owner presses **Resume** (see [Pausing](#pausing)) |
| Change how often it syncs | Board settings → sync → **Schedule** |
| Stop syncing | Board settings → sync → Danger zone → **Disconnect** |

## Before you start: the token

The sync reads ClickUp with a **ClickUp personal API token**. In ClickUp, open
**Settings → Apps** and generate one (it starts with `pk_`).

- The sync reads ClickUp *as the member who owns the token*. It sees only the lists and tasks
  that member can see.
- A personal token stops working when its member is deactivated in ClickUp. If the person who
  made it leaves, the sync stops too.
- So use a **dedicated ClickUp member for integrations** if you can, and make the token as
  that member.

Talaria seals the token in its database like every other secret. Nobody can read it back,
including you; the sync tab shows only its last four characters and the ClickUp member it
acts as. The token belongs to the board, not to you: if you leave the board, the sync keeps
running (the sync tab then warns that your token still drives it).

## Connecting a list

1. **Token.** Paste it and press **Continue**. Talaria checks it by asking ClickUp who it
   belongs to.
2. **List.** Pick the workspace, the space, the folder (or **Lists outside folders**), and the
   list.
3. **Preview.** Nothing has been written yet. Check:
   - **Status mapping.** Each ClickUp status, and the board column its tasks land in. Only an
     exact name match is filled in for you; every other status starts *unmapped*. Under each
     row a line says what that column means:
     - an **agent-start** column starts work for agents assigned to a ticket. The sync never
       adds agents, and it will not move a ticket into such a column while agents are assigned;
     - a **done** column closes the ticket when the task closes in ClickUp, and a workchain
       step hands on as if a person had closed it. Map closed statuses elsewhere if your
       workchains should wait for a sign-off in Talaria;
     - the sync moves tickets into **review** and **Blocked**, but never out of them.
       Neither move hands a workchain step on: only a done column does.
   - **The first tasks, as they would land**, with the column each goes to under the mapping
     as you have it now.
   - **Assignees, matched by email.** A ClickUp assignee whose email belongs to a member of
     this board is assigned. Anyone else is left unassigned and listed as a problem.
   - **New labels** the import would create from ClickUp tags.
   - **First import**: roughly how long it takes at your ClickUp plan's rate limit, and how much
     file data it copies.
   - **What carries over**: comments, attachments, closed tasks, and whether apps hear about
     synced tickets (off by default — see below).
   - **Sync**: Manual, every 15 minutes, hourly (the default), or daily.
4. Press **Connect and start syncing**. The first sync starts at once, and the chip on the
   board shows its progress.

Before a large first import, pause any app on the board that runs on new tickets, or leave
**Tell apps about synced tickets** off (the default) until the first import is done.

## What carries over

| From ClickUp | To the ticket |
| :--- | :--- |
| Name | Title (cut at 300 characters) |
| Description | Description (cut at 20,000 characters, with a link to the rest) |
| Status | The column from your mapping |
| Priority | Urgent, high and low as they are; normal and none become medium |
| Due and start dates | Due and start dates |
| Tags | Labels (the first 20) |
| Assignees | Board members with the same email |
| Sub-tasks | Sub-tickets under the top-level parent (Talaria nests one level) |
| Attachments up to 25 MB | Files on the ticket |
| Comments and replies | The ticket's discussion |

Not carried over: time tracking, custom fields, checklists, points, files attached to
comments, and ClickUp docs and goals.

**Comments** appear under their ClickUp author's name with a small **ClickUp** badge. They are never shown as a teammate's message, even when a teammate
has the same name, and nobody can edit them in Talaria. They arrive with their original time,
oldest first, and a reply threads under its comment. Imported comments notify nobody.

## When ClickUp and Talaria disagree

The sync remembers the last value it wrote for each field.

- If a field has not changed in ClickUp since then, your edit in Talaria stays.
- If it has changed in ClickUp, ClickUp's new value is written.
- Labels and human assignees merge: one you added in Talaria stays, and ClickUp's own
  additions and removals still land. Agents assigned in Talaria are never touched.

**Re-sync from ClickUp** on a ticket forces that one ticket to match ClickUp: title,
description, priority, dates, the column (through the rules above), and ClickUp's labels and
assignees replace the ticket's.

## Reading the chip

The chip beside the gear always says where the sync stands:

| The chip says | It means |
| :--- | :--- |
| **synced 12m ago** | All is well. Open it for the next sync time |
| **syncing** | A sync is running; the panel shows what it is doing, page by page |
| **not synced yet** | The first sync has not finished |
| **paused** | An owner froze the sync. The panel says who and when ("Paused by Dana Reyes · 5m ago"). Nothing syncs until an owner presses **Resume** |
| **sync overdue** (amber) | A scheduled sync is more than five minutes late. Press **Sync now** |
| **needs a new token** (red) | ClickUp refused the token — see below |
| **list not found** (red) | The list was deleted, or the token's member can no longer see it |
| **last sync failed** (red) | The panel says why |

A number on the chip is how many problems are open. When scheduled syncs are switched off on
your instance, the chip says so (amber, not "overdue") and **Sync now** still works.

The panel's **Last run** line is the latest run of any kind. After a ticket's **Re-sync from
ClickUp** or a mapping change it names that run ("latest run re-synced one ticket") rather
than reading as the board's last sync, which is the time on the **Last sync** line.

## Pausing

**Pause** freezes the sync. Use it while you fix the status mapping, or to take a last
import before you archive the board. While the sync is paused:

- no scheduled sync runs;
- **Sync now** and a ticket's **Re-sync from ClickUp** are off, for owners and editors
  alike. Talaria refuses them even if they are asked for another way;
- a sync that was running when you pressed **Pause** stops.

The chip, its panel and the sync tab tell everyone on the board who paused the sync and
when. You can still change the status mapping while the sync is paused. Tickets move to
the new mapping at once, without reading ClickUp. You can also change the schedule,
replace the token or disconnect.

A board owner presses **Resume**, in the chip's panel or in Board settings → sync, and
**Sync now** works again. A sync on a schedule runs within a minute of Resume and picks
up everything that changed in ClickUp while it was paused. After that it runs on its
schedule again. A manual sync does not run on Resume: press **Sync now** to catch up.

## Problems, and how to fix each

A problem is something the sync could not do. It is never a silent drop: each one names the
ticket and the ClickUp task, and clears itself on the next sync that resolves it.

| Problem | Fix |
| :--- | :--- |
| A status has no column | Map it in Board settings → sync, and **Save mapping**. The ticket waits in the intake column until then |
| An assignee is on Talaria but not on this board | Add them in Board settings → people, then **Sync now** |
| An assignee has no Talaria account, or ClickUp hides their email | Invite them to Talaria, or assign the ticket by hand |
| A file is over 25 MB | It stays in ClickUp; open it from the link |
| A move was held | The sync will not take a ticket out of review or Blocked, out of a column an agent is working in, or into an agent-start column while agents are assigned. Move it yourself if it should move |
| The task is synced to another board | One ClickUp task syncs to one board. See "Moving a list to another board" |

## When the token stops working

When ClickUp refuses the token — it was revoked, or its member was deactivated — the sync
stops before it reads anything. The chip turns red for everyone on the board, and the board's
owners and the instance's admins get one notification.

A board owner then opens Board settings → sync, pastes a new token, and presses
**Replace credential**. The mapping, the links between tasks and tickets, and the history are
all kept, and a sync starts at once (a paused sync stays paused). You can also replace a
working token the same way, for example to move to a dedicated integration member.

Admins can see the stored credential, and clear it, in **Admin → Secrets**.

## Archived, deleted and moved tasks

The sync **marks** tickets; it never deletes one. A ticket whose task was archived, deleted,
moved to another list, or put out of the token's reach in ClickUp shows a chip saying so, on
its card, its list row and its own page. A ticket you delete in Talaria is not made again.

## Disconnecting

Board settings → sync → Danger zone → **Disconnect**. Future syncs stop and the token is
cleared. Every imported ticket stays on the board, editable and owned by Talaria, and keeps
its **ClickUp** link. Connecting the same list again later re-attaches every ticket with no
duplicates, and the preview fills in your old mapping.

## Moving a list to another board

1. Disconnect the first board.
2. On the first board, delete the tickets or move them to the new board.
3. Connect the new board to the list, or press **Sync now** on it. A ticket you moved is taken
   over where it is; a deleted one is imported afresh.

## Rate limits

ClickUp limits each token per minute: 100 requests on the Free, Unlimited and Business plans,
1,000 on Business Plus, 10,000 on Enterprise. A first import needs about two requests per
task, so 2,000 tasks take about 40 minutes on the smaller plans. When the limit is reached,
the sync waits for ClickUp and says so ("Waiting for ClickUp's rate limit — resumes in 40s"),
then carries on where it stopped.

## Who can do what

| | Viewer | Editor | Owner |
| :--- | :-: | :-: | :-: |
| See the chip, the problems and a ticket's ClickUp link | ✓ | ✓ | ✓ |
| Sync now, Re-sync from ClickUp (not while paused) | | ✓ | ✓ |
| Open Board settings → sync (read-only for an editor) | | ✓ | ✓ |
| Connect, change the mapping or schedule, pause and resume, replace the token, disconnect | | | ✓ |

Admins see the stored token's health, and can clear it, in **Admin → Secrets**.

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
| Invite someone | Board settings → **people** → pick a person → choose **Editor** or **Viewer** |
| Allow agents on a board | Board settings → **agents** → pick the agents (or tick **Allow all agents**) → **Save** |
| Archive what's done | Board: settings → **general** → Danger zone → **Archive**. Ticket: right-click → **Archive** |
| See archived tickets | Flip the **archived** chip in the toolbar — archived tickets otherwise leave the board |
| Review agent work | Open the ticket in **Quality review** → read the result → **Approve** or **Request changes** |
| Work on my queue | **Work → Inbox** → the Boards tab shows **To triage**, **To review**, **Blocked** |

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

Open a ticket (click a card) for the full detail: description, attachments, comments with
`@`mentions (mentioned people get an inbox item), and the properties rail — status, priority,
color, assignees, effort, estimate, time spent, tokens (what the agent's work cost), dates,
parent/sub-tasks, dependencies (**Blocked by / Blocks**), labels, and watchers (**Watch** to
follow a ticket).

Right-click any ticket for the fast lane: move, priority, color, due date, assign to me,
archive.

**Muse** (in the ticket detail) edits fields from plain language — type
`urgent, due friday` — or select text while editing and ask for a rewrite; review the chips,
then **Apply**.

## Working with agents

| Agents can | Agents can't |
| :--- | :--- |
| Be assigned to tickets (by people) | Assign tickets — agent-created tickets land in **Inbox** for a person to assign |
| Move work **forward** only: into progress, into blocked, into review | Take a ticket out of blocked or review, or close one — a person signs off |
| Comment (with `@`mentions), even on closed tickets | Work archived tickets |
| Report a **Result** (outcome, resolution, error), time spent, and token cost | Edit archived boards |

Two approvals can wait for you inside a ticket:

- **Quality review** — **Approve** completes the ticket; **Request changes** sends it back to
  work. Optionally a judge model posts its verdict first (board settings → general →
  **QA judge**: advisory, or enforcing, which bounces failing work back to the agent up to
  three times before a human sees it).
- **Approve plan** — when an agent's coding work needs the go-ahead, the ticket shows what it
  plans to build; PRs it opens later appear here too (**View PR**).

## Who can do what

| | Viewer | Editor | Owner |
| :--- | :-: | :-: | :-: |
| See board, comment on open tickets | ✓ | ✓ | ✓ |
| Edit tickets, drag, bulk actions, save views | | ✓ | ✓ |
| Board settings (columns, labels, people, agents) | | ✓ | ✓ |
| Archive / restore | | ✓ | ✓ |
| Delete board, move it between teams | | | ✓ |

Sharing a board requires the person to have signed in once already. Team boards: team members
are editors, team owners own the board.

## Words Boards uses

| Term | Meaning |
| :--- | :--- |
| **Ticket** | A unit of work. (Developers meet the same thing as "task" in the API.) |
| **Reference** | Short code on every card, minted from the board's name — e.g. `QL-14` |
| **View** (saved) | A named filter+layout preset, shared with the board — a tab |
| **Status / column** | One step of the workflow; its category gives it meaning |
| **Effort / Estimate** | T-shirt size (XS–XL) and hours |
| **Watcher** | Someone following a ticket without being assigned |

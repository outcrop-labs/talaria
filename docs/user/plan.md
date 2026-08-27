# Plan

Plan is where you think work through **with an agent and your teammates** before it becomes
tickets. A plan is a multiplayer conversation with a **living document** beside it: as you
talk, the agent keeps the document current, and you can edit it directly. When the thinking
is done, **Draft tickets** turns the document into board tickets you review before anything
is created.

It is not a calendar — there are no dates or milestones here. The output of a plan is
tickets on a board.

Open: **Work → Plan** in the sidebar. Admins can hide Plan from individual people
(Admin → People).

## To…

| Do this | How |
| :--- | :--- |
| Start a plan | Pick an agent in the left rail → the `+` ("New plan: think it through, then draft tickets") → talk |
| Share a plan | The avatar stack (header) → **Share with** → pick a teammate. Everyone talks to the same agent and document; a green ring on an avatar means they're here now |
| Shape the document yourself | Click into it and type — it auto-saves. The agent's next sync starts from your edits |
| Rebuild it from the chat | **Sync from chat** — the agent rewrites the document from the conversation so far |
| Open it elsewhere | **Open ↗** — the document is a real file in Files |
| Draft tickets | **Draft tickets** (header) → pick planner, model tier, board, ticket template → **Draft tickets** |
| Review the drafts | The modal walks you draft by draft: edit title/priority/effort, wire **Blocked by** dependencies, untick any you don't want → **Create all** |
| Follow up later | Closed the modal? Nothing stopped — **Drafting…** continues server-side; reopen any time. The button reads **Review drafts** when they're waiting |

Created tickets land in the board's **inbox**, unassigned, with dependencies wired between
them.

## The pieces

| Piece | What it is |
| :--- | :--- |
| **Conversation** | The multiplayer chat — everyone shares one agent |
| **Living document** | The plan taking shape beside it: outline, goals, scope, decisions. A real, versioned file |
| **Template** | The structure the document starts from — a plan template of yours, or **Automatic** (the agent's bound one) |
| **Model tier** | Which tier runs this chat and the rewrites |
| **Draft job** | The server-side run that reads the conversation and proposes tickets — it survives a closed tab |

The agent plans, it doesn't act: inside a plan it creates nothing — no tickets, no files.
Turning the plan into tickets is your move.

## Who can do what

Starting plans is a permission (**Create plans**, on for members by default). Sharing a plan
is the owner's call; collaborators can remove only themselves. You can draft onto any board
where you're an owner or editor.

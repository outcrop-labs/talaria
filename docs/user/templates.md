# Templates

Templates are the markdown skeletons work starts from — two kinds only, **Tickets** and
**Plans**. A template never spawns anything by itself: it seeds the description of every new
ticket on a board and the living document of every new plan, and the QA judge scores
agent-drafted tickets against its sections. Wherever one applies, the same resolution order
holds: **explicit pick → agent binding → board default → freeform**.

Open: **Manage → Templates**. Editing needs the **Manage templates** permission; see
[Who can edit](#who-can-edit).

## The two kinds

| Tab | What it seeds |
| :--- | :--- |
| **Tickets** | Every ticket description: board defaults, agent bindings, and the QA judge scores against its sections |
| **Plans** | A new plan's living document, and how the agent rewrites it as the conversation grows |

## To…

| Do this | How |
| :--- | :--- |
| Create one | **New ticket template** / **New plan template** on the left → it opens immediately |
| Edit one | Pick it on the left → set **Name**, write the skeleton, add **Agent guidance** → **Save** |
| Delete one | Trash icon on the record, or right-click the row → **Delete**. Boards and agents bound to it fall back down the template chain |
| See what changed | The **Version history** toggle — every changed save snapshots the skeleton |
| Use one for tickets you draft from a conversation | The drafting flow's **Ticket template** picker — **Automatic (agent → board default)** follows the chain, or pick explicitly |
| Use one for a plan | The plan header's **Template** picker — **Automatic** uses the agent's bound plan template, or pick from the library |
| Bind one to a board | Board settings → ticket templates section → **Manage library →** pick a default |
| Bind one to an agent | The agent's **Templates** section → pickers **Tickets** and **Plan documents** |

## Agent guidance

The **Agent guidance** field is prompt-only: it travels with the template into the model's
instructions but is never shown on the ticket or plan itself — for example
*"Always fill acceptance criteria; keep Out of scope honest."*

## Where a template applies

| Where | What happens |
| :--- | :--- |
| New ticket on a board | The board's default seeds the description; an agent's own binding wins |
| Agent-drafted tickets | Formatted on the board's ticket template — same chain, assignee's binding first |
| The QA judge | Scores a draft against the template's sections as an objective rubric |
| New plan | The living document starts as the template skeleton, filed in Files under the plan agent's cabinet as *Plan — \<title\>* |

## Who can edit

The page shows **Admins only** to anyone without the admin role. Editing needs the grantable
**Manage templates** permission ("Edit the org-wide ticket and plan templates everyone starts
from") — admins switch it on per person in **Admin → People**. Every template picker in the
app reads the same library, so a granted member's templates show up everywhere.

## Words Templates uses

| Term | Meaning |
| :--- | :--- |
| **Skeleton** | The markdown body of the template — the part that seeds tickets and plans |
| **Agent guidance** | Prompt-only instructions that ride along with the skeleton |
| **Binding** | A board's or agent's standing pick of a template; an agent's binding beats the board's default |
| **Freeform** | No template at all — you write from scratch |
| **Role templates** | A different thing: starter personas for *agents* on the Agents page. Nothing to do with this library |

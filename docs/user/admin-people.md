# Admin: People

Everything about who's in the org lives in **Admin → People**: invites, roles, what each
person may do, and which surfaces they can reach. Two roles exist — **Admin** and **Member** —
and admins hold every permission unconditionally; there's nothing extra to grant them. What
you're really managing is two different axes: **permissions** (what people may do) and
**views** (which surfaces they can reach at all).

Open: user menu → **Admin** → **People**.

## Invite someone

**Invites** at the top of the tab. Type their email → **Invite**: they get a join link and
are admitted the moment they sign in with Google on that address. Invites expire after two
weeks; re-inviting re-issues a fresh link; revoking shuts the door instantly. Needs the Email
provider set on the Organization tab.

Each invite row shows its state: `accepted` · `pending` · `expired` · `revoked`.

The invite is bound to the email address — signing in with a different account won't use it.
Self-service joins are possible too, via verified **Email sign-up domains** on the
Organization tab.

## Set someone's role

The role select on their row: **Member** / **Admin**. Two guards: you cannot demote yourself,
and an admin pinned via `AUTH_ADMIN_EMAILS` can't be demoted from here at all.

To offboard someone, demote to Member, revoke their grants and views on the row, and revoke
any pending invites. There is no suspend or delete — the audit trail and everything the
person produced stay.

## What each person may do

On each member's row, the mono word **can** is followed by one chip per permission. A
filled chip means allowed; a dot marks one you've overridden from the default — click a
chip twice to put it back. Hover any chip for its one-line hint.

**Member defaults** (top of the tab) sets the org-wide baseline — what every plain member may
do out of the box. The chips group by area (Agents, Work, Comms, Content, Models); a dot
marks a default you've changed from Talaria's shipped baseline. Per-person chips override it
in either direction.

### The catalog

| Permission | Area | Members by default | What it controls |
| :--- | :--- | :--- | :--- |
| **Manage agents** | Agents | ✗ | Hire, retire, and configure org agents: souls, skills, crons, start/stop. Agent secrets and infrastructure stay admin-only. |
| **Run research** | Work | ✓ | Start research runs (recon, briefs, expeditions). |
| **Create plans** | Work | ✓ | Start plan conversations and their living documents. |
| **Create boards** | Work | ✓ | Create new boards. Working on boards they belong to is membership, not this. |
| **Create channels** | Comms | ✓ | Create persistent channels. Joining and posting is membership. |
| **Start relays** | Comms | ✓ | Spin up relays — ephemeral working groups that conclude and archive. |
| **Edit knowledge** | Content | ✓ | Create and edit knowledge docs (per-doc/space ACLs still apply). |
| **Curate knowledge** | Content | ✗ | Create spaces and mark docs OFFICIAL — content that grounds every agent. |
| **Create documents** | Content | ✓ | Create documents and artifacts. |
| **Publish to the web** | Content | ✗ | Make artifacts PUBLIC — reachable by anyone with the link, outside the org. |
| **Upload files** | Content | ✓ | Attach files and images to chats, channels, and tickets. |
| **Manage templates** | Content | ✗ | Edit the org-wide ticket and plan templates everyone starts from. |
| **Mint API keys** | Models | ✗ | Mint personal LLM-gateway API keys for external tools. |

## Which surfaces each person reaches

The **views** box on each member's row mixes two kinds, and they behave oppositely:

| Kind | Default | Picking one |
| :--- | :--- | :--- |
| **Work views** (Comms, Plan, Boards, Research, Knowledge, Files) | Allowed | Restricts — hides that surface |
| **Manage views** (Agents, Models, MCP, Templates, Agent Studio, Observability, Apps — and every app surface) | Denied | Grants — shows that surface |

Empty means all work views and no manage views. Apps are explicit-grant the same way: each
app view appears here and is allowed per person. Admins always have full access.

The **agent access** box on the row bounds which agents the member can work with
(**All agents** if empty).

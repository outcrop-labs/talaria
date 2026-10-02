# Permissions & access — who may do what

Four mechanisms, each with one job:

1. **Roles** — `admin` / `member`. Admins hold every permission and see every view; the Admin
   console itself is role-locked.
2. **Views** — which surfaces a person can reach at all.
3. **Permissions** — what a person can *do* (13-entry catalog).
4. **Teams** — first-class org principals (people + agents) that expand at auth time. A team's
   view grants, permission overrides, and MCP tool rules apply to its human members; its agent
   members inherit the team's resource ACL and MCP assignments. Manage → Teams is the home;
   resource ACLs accept a team the same way they accept a person.

Resource-level ACLs (board membership, KB editors, plan/research/channel shares, agent managers)
stay on the resources themselves: a permission says what you CAN DO, an ACL says what you can do it
TO. Adding a team to a resource does not fan out member rows — membership is expanded when the
ACL is checked, so roster changes apply immediately.

## Views (Admin → People, one checklist)

- **Work views** (Comms, Plan, Boards, Research, Knowledge, Artifacts) default **allowed**;
  denials are stored per person.
- **Manage views** (Agents, Teams, Models, MCP, Templates, Observability, Apps) default **denied**;
  explicit grants are stored per person. View access opens the door; permissions still gate the
  actions inside.
- **App views** (`/x/<slug>`, `/x/<slug>/manage` — tagged `app` in the checklist) behave like
  Manage views: **explicit-grant only**. Enabling an app gives members nothing until an admin adds
  it per person.

Denied views aren't just hidden: the route bounces, the nav omits them, and the APIs that power
them enforce the same resolution server-side (`requireView`).

A team may carry the same two arrays (`denied_views`, `allowed_manage_views`). Effective views:

- work denials = the person's denials ∪ every team they belong to;
- manage grants = the person's grants ∪ every team they belong to.

Admins set team view grants on Manage → Teams (not the roster owner).

## The permission catalog

13 permissions in five groups (`api/crates/talaria-permissions/src/lib.rs` is the catalog; the
groups are what Admin → People renders):

- **Agents** — `agents.manage` (the FLEET: hiring, LLM endpoints, MCP servers, the role library.
  Changing one agent is its managers' — see below).
- **Work** — `research.run`, `plans.create`, `boards.create`.
- **Comms** — `comms.channels`, `comms.relays`.
- **Content** — `kb.edit`, `kb.official`, `artifacts.create`, `artifacts.publish`, `files.upload`,
  `templates.manage`.
- **Models** — `models.mint-keys`.

Each ships a sensible member default; the ones that are **off** by default are `agents.manage`,
`kb.official`, `artifacts.publish`, `templates.manage`, and `models.mint-keys`.

**Resolution, most specific wins:**

1. per-user overrides (allow or deny),
2. team overrides (`bool_or` across teams the person belongs to — any allow wins; a perm only
   denied across teams is denied),
3. org-wide member defaults (Admin → People → Member defaults),
4. the catalog's shipped defaults.

Admins hold everything unconditionally. The Admin → People per-person chips show effective state
and where it came from (override dot vs inherited). Team chips on Manage → Teams show the team's
own overrides, not a member's effective set.

## Agent managers

An agent is somebody's. **Only its managers may change it** — identity and role, the soul and
model config, skills, memory, schedules, secrets, MCP binds, workbench repos, and the whole
lifecycle (start/stop/restart/roll/retire/delete). Everyone else who can see the agent gets the
same manage surface read-only.

- **Admins manage every agent**, named or not. An agent whose manager leaves the org must not
  leave with them, and `Managers` is where an admin hands it to someone else.
- **Being named a manager is the whole grant.** It needs no `agents.manage`, and it opens the
  `/agents` view by itself — a grant that left someone unable to reach the surface where their
  agent lives would not be one. The roster then shows *their* agents; `agents.manage` is what
  widens it to the fleet (and such a reader sees other agents read-only).
- **`agents.manage` is the fleet, not any agent.** It still gates hiring, federating, LLM
  endpoints, MCP servers, role templates, the shared skills root and the fleet-wide schedules —
  none of which belong to one agent. It no longer confers the right to rewrite an agent
  somebody else owns.
- **Whoever hires an agent manages it**, and a personal assistant is managed by its human
  (`agent_defs.owner_user_id` still answers for an assistant that predates its manager row).
- **The roster is never empty.** The PUT refuses a manager list of zero, so nobody can lock
  themselves out of their own agent: hand it over by adding the new manager in the same request
  that drops yourself.

The rows live in `agent_managers` (agent × user). Where this is asked in code:
`talaria_agent_managers::manages_agent` and the route guard
`talaria_session::require_agent_manager` — the per-agent twin of `require_perm`, rendered in the
generated reference as `session` + `agent-manager`. The roster is
`GET`/`PUT /api/fleet/defs/{id}/managers`, and the Agents → Manage modal's **Managers** tab.

On upgrade, every personal assistant was written in under its owner and every other agent under
the admins who could already change it, so nothing an admin could do before the upgrade stopped
working after it. Admins added later hold reach by role rather than by a row.

## Personal assistants

A personal assistant is an agent bound to one human (its owner). It holds no roles, views, or
permissions of its own; each surface instead answers *"what would the owner's reach allow, minus
the destructive parts"*:

- **Boards** — the board's agent allow-list stays authoritative and restrictive by default.
  What inheritance buys is the grant path, not a bypass: on any board its owner can *read*, the
  assistant adds itself (`POST /api/boards/{id}/agents/self`, one step); on boards the owner
  cannot see it files a request the board's editors approve or decline
  (`/api/boards/{id}/agent-requests` — also a `board_access` approval in the editors' queue).
  It may remove only its own row; the editor policy PUT remains the only way to touch anyone
  else's.
- **Knowledge & artifacts** — `can_read_agent` mirrors the owner for *reads*: private docs,
  spaces, and artifacts the owner owns open to their assistant (retrieval already served them;
  now the file plane agrees). Edit stays grant-only (`can_edit_agent`), and sharing, brain
  routing, and officialness stay human-only routes.
- **Destructive actions are not inherited.** Agents never assign or sign off tickets, never
  delete boards/tickets/members, and a personal assistant's outbound mail and invites wait for
  an approval card. Inheritance is read + draft reach, not authority.
- An admin's own assistant can be marked **elevated** (Admin → People) for an org-wide view, and
  `GET /api/agent/whoami` introspects any agent's effective reach (identity, boards with *why*,
  guardrails, pending requests).

## Who owns what agents make

When an agent creates something — a knowledge doc or space, a document or file, a saved image,
a research report, a workbench plan — the item's owner is **the human responsible for the agent
run**, not the agent:

1. a **personal assistant**'s output belongs to its owner (and starts private to them, as
   before);
2. an **org agent working mid-chat** attributes to the human it is answering;
3. anything else the agent makes on its own (crons, standalone calls) attributes to **the admin
   who hired it**;
4. an agent nobody can trace stays ownerless.

Default visibility is unchanged — an org agent's output remains org-readable; ownership adds
*control* (share it, flip it private, set policy), not secrecy. A research run keeps its
org-ness either way: an org agent's findings stay org-visible and ambient-indexed however
owned, while a person's own run (or their assistant's) stays private with its share grants.

Two consequences worth knowing:

- **Governance follows the owner.** On a human-owned item, the owner alone governs — *admins
  included* do not get `can_govern` there (`can_govern`'s owner arm is exclusive by design).
  The owner can always re-share or hand the item off by changing visibility.
- **Read reach is not ownership.** The ladder stamps owners; it never widens what the *agent
  itself* may read. An org agent's read reach stays its own (org/public + grants); only a
  personal assistant reads through its owner's eyes.

Going forward only: items made before attribution (ownerless, governed by admins + the agent's
allow-list) stay exactly as they were.

## Enforcement in code

Every API route speaks one dialect ([API-CONVENTIONS.md](./API-CONVENTIONS.md)):
`requireUser` / `requireAdmin` / `requirePerm(perm)` / `requireView(view)` from
`server/api-guard.ts` (the Rust twin is `talaria_session`'s `require_*`, which answer
`Result<_, Response>` and propagate with `?`), then resource ACL checks where the resource
carries its own. UI affordances
follow `useHasPerm` / `useDeniedViews` — but the server is the authority; hiding a button is
courtesy, the 403 is the contract.

## Related

- Agent allow-lists (which agents a member may *use*) live on the person in Admin → People. Using
  an agent and managing it are separate questions: a use grant has never implied the right to
  rewrite how the agent works.
- MCP tool access (per-agent, per-person, and per-team, per server) is its own governed system:
  [MCP.md](./MCP.md). Manage → Teams is where a team's platform views, permission overrides, and
  roster (people + agents) live.
- Sensitive mutations audit-log with a canonical actor; see the audit trail on /observability.

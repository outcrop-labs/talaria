# Permissions & access — who may do what

Three mechanisms, each with one job:

1. **Roles** — `admin` / `member`. Admins hold every permission and see every view; the Admin
   console itself is role-locked.
2. **Views** — which surfaces a person can reach at all.
3. **Permissions** — what a person can *do* (13-entry catalog).

Resource-level ACLs (board membership, KB editors, plan/research shares, personal-agent ownership)
stay on the resources themselves: a permission says what you CAN DO, an ACL says what you can do it
TO.

## Views (Admin → People, one checklist)

- **Work views** (Comms, Plan, Boards, Research, Knowledge, Artifacts) default **allowed**;
  denials are stored per person.
- **Manage views** (Agents, Models, MCP, Templates, Observability, Apps) default **denied**;
  explicit grants are stored per person. View access opens the door; permissions still gate the
  actions inside.
- **App views** (`/x/<slug>`, `/x/<slug>/manage` — tagged `app` in the checklist) behave like
  Manage views: **explicit-grant only**. Enabling an app gives members nothing until an admin adds
  it per person.

Denied views aren't just hidden: the route bounces, the nav omits them, and the APIs that power
them enforce the same resolution server-side (`requireView`).

## The permission catalog

13 permissions in five groups (`server/permissions.ts` is the catalog; the groups are what Admin →
People renders):

- **Agents** — `agents.manage`.
- **Work** — `research.run`, `plans.create`, `boards.create`.
- **Comms** — `comms.channels`, `comms.relays`.
- **Content** — `kb.edit`, `kb.official`, `artifacts.create`, `artifacts.publish`, `files.upload`,
  `templates.manage`.
- **Models** — `models.mint-keys`.

Each ships a sensible member default; the ones that are **off** by default are `agents.manage`,
`kb.official`, `artifacts.publish`, `templates.manage`, and `models.mint-keys`.

**Resolution, most specific wins:**

1. per-user overrides (allow or deny),
2. org-wide member defaults (Admin → People → Member defaults),
3. the catalog's shipped defaults.

Admins hold everything unconditionally. The Admin → People per-person chips show effective state
and where it came from (override dot vs inherited).

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

## Enforcement in code

Every API route speaks one dialect ([API-CONVENTIONS.md](./API-CONVENTIONS.md)):
`requireUser` / `requireAdmin` / `requirePerm(perm)` / `requireView(view)` from
`server/api-guard.ts`, then resource ACL checks where the resource carries its own. UI affordances
follow `useHasPerm` / `useDeniedViews` — but the server is the authority; hiding a button is
courtesy, the 403 is the contract.

## Related

- Agent allow-lists (which agents a member may use) live on the person in Admin → People.
- MCP tool access (per-agent and per-person, per server) is its own governed system:
  [MCP.md](./MCP.md).
- Sensitive mutations audit-log with a canonical actor; see the audit trail on /observability.

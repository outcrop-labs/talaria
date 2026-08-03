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

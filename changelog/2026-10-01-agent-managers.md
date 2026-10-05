- **Agents have managers.** An agent is now somebody's: only the people named
  on it — and admins — can change it. That covers everything the Manage modal
  reaches (identity and role, the soul and model config, skills, memory,
  schedules, secrets, MCP binds, workbench repos) and the whole lifecycle
  (start/stop/restart/roll/retire/delete). Before this, one permission said
  both "may run the fleet" and "may rewrite any agent in it", so every
  `agents.manage` holder could rotate another team's agent's secrets or retire
  it. `agents.manage` keeps the first job — hiring, federating, LLM endpoints,
  MCP servers, role templates, the shared skills root, fleet-wide schedules —
  and has stopped doing the second.

  Being named a manager is the whole grant: it needs no `agents.manage` and it
  opens the `/agents` view by itself, where the roster shows that person's own
  agents (a fleet-wide reader still sees everything, read-only on the agents
  they do not manage). Whoever hires an agent manages it; a personal assistant
  is managed by its human. A new **Managers** tab on the Manage modal is where
  an agent changes hands, and it refuses an empty roster — hand it over by
  adding the new manager in the same request that drops yourself. Admins keep
  reach over every agent, so one never becomes unreachable when the person who
  owned it leaves. New: `agent_managers`, `GET`/`PUT
  /api/fleet/defs/{id}/managers`, the `require_agent_manager` route guard
  (`session` + `agent-manager` in the generated reference), and a
  `agent.managers_set` audit entry carrying both rosters.

  Two things this fixes on the way past. Editing an agent's SKILLS was granted
  by `user_agent_access` — a "may use this agent" grant conferring the right to
  rewrite how it works — and that arm is gone; skills follow the agent's
  managers like everything else. And `talaria-skill-access` carried an
  `OWNS_AGENT` OnceLock meant to let a personal assistant's human edit its
  skills, which nothing ever set, so that arm had never once answered true; it
  is deleted rather than wired, because the manager lookup reads the owner
  column directly.

  On upgrade every personal assistant is written in under its owner and every
  other agent under the admins who could already change it, so nothing an admin
  could do before stops working after.

  Verified against a real instance, not a fixture: the `talaria-dogfood`
  database (4 people, 1 admin, an org agent and a personal assistant) restored
  into an isolated worktree stack, migrated, and driven over HTTP with minted
  sessions. The backfill wrote the assistant under its owner and both agents
  under the admin; a member named manager of one agent could PATCH it, read its
  crons and secrets and drive its lifecycle, saw only that agent on
  `/api/fleet/defs`, and reached `/agents` with no view grant; a member not
  named got 403 from every one of those and an empty roster; the manager PUT
  refused an empty set and a non-existent user id. Five `#[ignore]`d live-DB
  tests cover the gate, the owner arm, the handover, the roster narrowing and
  the view grant; `bun run check`, `bun run typecheck` and the api compile are
  green.

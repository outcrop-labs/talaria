- **One Developer Agent switch replaces the workbench settings.** An agent's
  Summary tab has a single on/off toggle. Turning it on gives the agent the dev
  sandbox, Oh My Pi as its coding harness, and the Workbench tools (`doctor`,
  `start_job`, `finish_job`, …), then rolls the agent so the change lands.
  Repo grants still appear under the toggle. The switch is now the only grant
  of the `workbench` MCP server: the registry derives it from
  `agent_defs.developer`, ignores assignment and team rows for it, and the MCP
  page no longer offers access controls for it. Before this, an agent could be
  set to Workbench **On** with a harness and repos and still have no
  `start_job`, because the tools needed a separate grant on another admin
  screen, and changing the workbench fields never rolled the agent. That is
  why dev jobs never started on talaria-packledger. Oh My Pi is the only
  harness: opencode, Pi, the profile registry, department/role auto-attach,
  the harness picker, custom harness definitions (`/api/workbench` and
  `/api/workbench/harnesses`), `defineWorkbenchHarness`, and per-agent effort
  model overrides are gone. The org's Workbench model roles become omp's own
  roles (code-standard → default, code-light → smol, code-heavy → slow and
  plan, via `PI_*_MODEL`), so omp chooses between them as it works. Effort
  still decides planning and the RAM reserve. `TALARIA_WORKBENCH_IMAGE` sets a
  custom image for Developer Agents. The migration adds `developer` and turns
  it on for agents that had Workbench **On** or a Workbench MCP grant; the old
  columns and tables stay, unread, for a later release to drop. Verified: `bun run check`; svelte-check (0
  errors) and 1304 ui tests; `cargo fmt`, `clippy -D warnings`, and 622 tests
  across the 14 touched packages via `bun run gate`; a fresh migration replay
  (386 statements, snapshot matches, second run applies 0); the backfill
  against rc-shaped rows (Workbench on → on, auto with a workbench grant → on,
  auto alone and off → off); and the new live test `developer_agent_live` (a
  leftover grant row grants nothing, the switch alone renders and authorizes
  the workbench, off revokes both).

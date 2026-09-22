- **Teams are first-class.** They are no longer a boards-only grouping:
  Manage → Teams (`/teams`) is a LibraryPane of org teams (people + agents),
  with admin view grants, permission overrides, and MCP tool rules on the
  team itself. Adding a team to a channel, plan, research run, KB doc, or
  artifact expands at auth time — roster changes apply without rewriting
  grants. Boards still use `boards.team_id` as ownership. Verified: `bun run
  check`; `cargo clippy -D warnings`; `kb::perms` tests (10); minted an
  admin session on the `teams` worktree stack (`:5305`) and created
  Engineering via `POST /api/teams`, granted `/mcp` + `kb.official` via
  `PUT /api/teams/{id}/access`, then opened Manage → Teams (nav, picker,
  people/agents/access chips) and MCP → Manage access (Teams section).

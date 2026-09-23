- **api/src is organized one dir per subsystem.** The flat layout — 217 route
  modules and 149 top-level engines in two undifferentiated piles — is gone.
  `api/src/routes/` now carries 23 group dirs mirroring the `docs/api` groups
  (account, boards, comms, fleet, knowledge, …), the map inverted from the
  reference's own Source links; the router table's handler paths are
  group-qualified, so the table names the system. The engine families fold
  into dirs with their prefixes dropped (`crate::fleet_render` →
  `crate::fleet::render` — the shape `fitness/` and `runs/` were born with):
  `fleet/`, `google/` (gmail included), `mcp/`, `model/`, `inbox_focus/`,
  `kb/`, `workbench/`. Pure moves and path rewrites — no behavior change;
  module names stay (docs/api links re-pointed, provenance headers intact),
  `module_inception` is allowed at the crate level for the flagship-module
  shape (`routes/agents/agents.rs`), and the top level keeps its true singles
  (101 files, down from 149).

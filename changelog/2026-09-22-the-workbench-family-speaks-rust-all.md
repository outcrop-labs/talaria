- **The workbench family speaks Rust — all seven routes, github's REST
  half, and the MCP dispatcher the fleet's agents speak.** The profile
  registry (member reads mask env values to `•••`, the image/mounts
  infrastructure fields stay admin-only), the per-repo git flow, the org
  GitHub connection's read plane, the harness registry (declarative
  custom definitions stored in probed zod shape order, builtins
  undeletable), the human side of workbench jobs (the ticket strip, the
  approve/reject gate through board editors, merge-to-testing through
  the engine with both failure flavors folded into the TS `.catch()`'s
  one 400), the repo-creation approval queue, and the per-agent repo
  grants. Verified by a 123-case byte-diff against TS — the largest
  family harness yet: gate tables for anon/member/admin, the full zod
  400 tables for all seven bodies, restore-safe mutation cycles, twelve
  SQL-seeded jobs with fixed ids and timestamps (approve/reject ±note,
  the re-approve's `job is started`, the null-task 403, both merge
  refusals on the unconfigured connection), and the repo-request reject
  ladder. The diff caught three things the unit tests could not: the
  THROWN-500 shape — defineApi has no catch, so a TS handler that throws
  answers SvelteKit's text/plain `Internal Server Error`, and all 498
  port sites mirroring a TS throw were swept to that shape port-wide;
  the profile autoAttach wire, which is the stored column's jsonb
  canonical order, not the parser's shape order; and the harness list's
  custom order, which no runtime orders at all (no ORDER BY) — the
  harness pins the builtin prefix and compares the customs as a set.

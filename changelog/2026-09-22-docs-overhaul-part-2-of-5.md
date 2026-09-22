- **Docs overhaul, part 2 of 5 — the generated references** (`bun run docs:api`, wired into
  `bun run check` as `--check`). New `scripts/gen-docs.mjs` extracts the HTTP API reference from
  the route sources — 214 routes, 343 (path, method) rows → `docs/api/` (23 group files + index
  with the auth legend): path, method, guard class (session/admin/perm/view/agent/dual/fleet/
  bearer-key/public), parseBody field tables (inline and named zod schemas, z.union variants),
  literal statuses, SSE/audit markers, and a heuristic Returns column that prints `…` rather than
  guess. The CLI reference (`docs/CLI-REFERENCE.md`) imports the real command tree — the same
  declarations `--help` renders — plus a hand-written guide (`docs/CLI.md`). Prose notes live in
  the source as `// doc:` runs (21 routes seeded); otherwise a route's own leading comment is the
  note. Guard vocabulary is closed: an unrecognized `await X(request)` renders `unknown(X)`, never
  a false `public` (this caught `actingUser`/`taskActor`/`commentReader` during the audit). CI now
  runs `bun run check` instead of the bare invariants script, so every gate added to the chain
  actually reaches CI.

- **CI checks the surface that moved, not the whole tree.** A UI change no
  longer re-runs the Rust api's `fmt`/`clippy`/`test`, a desktop change does not
  re-run the UI suite, and a docs-only push runs the invariants job and nothing
  else — decided per job, inside the run, by a new `changes` job in
  `.github/workflows/ci.yml` (`.github/workflows/migrations.yml` gets the
  equivalent trigger filters, its only inputs being the migration array in
  `ui/src/server/db/pg.ts` and the snapshot beside it). The invariants job stays
  unconditional on purpose: it is seconds long, needs no install, and a
  "docs-only" change is exactly what breaks a doc link. Trigger-level `paths:`
  are still out — a workflow-level filter is as coarse as the file, and a
  filtered-out required check would report "pending" forever — and every unknown
  resolves to the safe side: a first push, an unresolvable diff range, or a
  `release.yml` call runs EVERY surface, because a publish is never a partial
  gate. Verified: the decision logic was extracted verbatim and run against a
  fixture repository for every path (ui-only, api-only, desktop-only, docs-only,
  workflow-file, zero-before push, called-from-release, unresolvable range), each
  producing the intended set; and `main`'s protection was read back to confirm it
  requires a review and **no status checks**, so a skipped job cannot wedge a
  merge the way a filtered-out required check would. The skip path itself becomes
  observable on the first push to main that touches no code.

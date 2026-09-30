- **A malformed ticket subject answers 403, not 500.** `runs.subject_id` is
  TEXT while `tasks.id` is uuid, so binding a subject that is not uuid-shaped
  into `where id = $1::uuid` makes Postgres raise rather than simply not
  match — the bind-cast trap in `docs/RUST-MIGRATION.md`. The board hop added
  in #502 would therefore have answered 500 for a row the old code refused
  cleanly. The shape is now checked before the cast can see it, and anything
  that is not uuid-shaped is treated as "no such task", which is what it is.
  Checked in Rust rather than by adding a uuid dependency or comparing
  `id::text` (which would drop the primary-key index). Verified:
  `bun run check`, `cargo fmt --all --check`, and a unit test covering the
  deterministic non-v4 ids work sessions actually use, a hostile subject, and
  the near-miss shapes either side of 36 characters.

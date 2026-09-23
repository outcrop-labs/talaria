- **The port's text-vs-typed bind crashes: three statements that could never
  execute, found by auditing every query in the API.** sqlx declares each
  bind's wire type from the Rust value — a `String` bind crosses as TEXT —
  while the TS original's postgres.js let Postgres infer types per call.
  A text bind ASSIGNED into a typed column is coerced and harmless; a text
  bind COMPARED against one is a prepare-time `operator does not exist`
  that nothing catches at authoring time. Three carriers had been live-dead
  since the cutover: the 15-minute RAG sweep's four window queries all
  compared the ISO watermark against `timestamptz` columns, and
  `.unwrap_or_default()` ate the error — the sweep indexed nothing for
  weeks while its watermark advanced, silently burning every window (the
  mark needs a one-time reset on each instance so the repaired sweep
  re-covers them; indexing is content-hash idempotent); the daily brief's
  48h recency read 500'd on every still-current brief; the reindex run's
  artifacts page died whenever an artifact existed (`any($1)` of text ids
  against uuid). The casts now ride the binds (`$1::timestamptz`,
  `$1::uuid[]`), a live-DB suite (`api/tests/typed_binds.rs`, `#[ignore]`d)
  executes the three real carriers against the real schema, and reverting
  any one cast has been shown to fail its test with the exact fleet error.
  The same audit left the rest of the corpus clean: 910 of 968 statements
  proven legal by EXPLAIN against the live schema, the bind-cast and
  TS-parity sweeps converging on exactly these three, and the row-mapping
  sweep on one latent nullability fix — the inbox undo read mapped a
  nullable `outcome` column into a non-Option type, which now tolerates a
  completed-without-outcome row instead of 500ing the undo.

- **The fitness plane speaks Rust — the probe/eval/adversarial battery,
  its run engine, its archive, and the whole verdict surface.**
  `/api/admin/model-fitness` crossed as one route over six engine
  slices: the matrix (every gateway model against every harness),
  capabilities, health, value (the money view — workload shares, the
  USD-per-ready-run arithmetic), detail with its live-run block and
  stale-run sweeping, estimate (fixture arithmetic over the last
  archived report), and transcripts, plus the action union —
  start/stop/clear/forget with the arming ladder (role checks, the
  adversary-may-not-be-the-candidate refusal, concurrency 1..8).
  Three port principles came out of it and now hold crate-wide. Stored
  jsonb never rides a typed struct to the wire: Postgres keeps object
  keys in its own canonical order and TS passes the parsed blob
  through untouched, so every stored-object passthrough (the detail
  `record`, run rows, both views' `index`) rides the raw
  `serde_json::Value` while typed parses serve decisions only.
  `JSON.stringify` prints whole floats as integers (`1`, not `1.0`),
  so responses go through a `js_numberify` walk. And serde_json's
  default float parse is best-effort — it can land one ULP off the
  parsed digits, which a 456KB otherwise byte-identical body proved on
  a stored `0.9090909090909091` — so the `float_roundtrip` feature is
  on for every float the API parses. Verified by a 27-case byte-diff
  against TS (all seven views × archived/unarchived/missing models,
  the estimate query-param folds, the anon/member gates, and eleven
  POST bodies): 21 byte-identical, 6 exactly the app-harness boundary
  (app-shipped harnesses are TS-only by rule 10, so their counts and
  arithmetic differ on any install with an enabled app — recorded
  under divergences), 0 unexplained. The start body's zod union was
  probed against the verbatim TS schema: any parse failure collapses
  to the union's `"Invalid input"`, and only bound failures surface
  the arm's own sentence, in field order. The diff also found a TS
  bug, not a port one: `clear` 500s on TS for every model
  (`setSetting(key, null)` writes SQL NULL into a NOT NULL column) —
  Rust keeps the working behavior and the bug is flagged for the TS
  side.

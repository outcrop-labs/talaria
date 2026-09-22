- **Board column edits write again, and the DEK rotation sweep can run.**
  The fourth eruption of the port's typed-bind crash class, reported live:
  editing anything about a board column — label, color, category, and the
  agent-start flag that says where agents pick work up — 500'd on every
  attempt, first edit to the last. `update_status` resolves the row by key
  and reads it back `id::text`, then built the dynamic
  `update board_statuses set … where id = $N` and bound that string as TEXT
  against the uuid column: prepare resolves `uuid = text`, the operator does
  not exist, and the route answers 500 before anything moves. The cast rides
  the final bind now (`where id = $N::uuid`), matching the delete path that
  had it right all along. The same sweep found the identical shape in
  `secret_rotation`: every `CIPHER_TARGETS` pk is non-text (three uuid, one
  integer), so a DEK rotation would have died on its first row — the
  re-encrypt update now compares `"pk"::text = $N`, the same text the sweep
  reads them as. Proof in `api/tests/typed_binds.rs`'s house form: a
  live-DB test drives the real `update_status` (materialize, then the
  dynamic statement) twice with different fields and reads the rows back
  changed, plus a pinned unit test on the extracted `pk_where` builder; a
  sweep of every other `id/_id/_at = $N` site in `api/src` against
  `information_schema` found the rest text-typed or already cast.

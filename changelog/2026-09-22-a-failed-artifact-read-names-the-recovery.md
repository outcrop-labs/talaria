- **A failed artifact read names itself and how to recover.** GET
  `/api/artifacts/{id}` answered a transient read with a bare `Internal
  Server Error` and a log line that named neither the artifact nor which
  read failed, so `get_document` burned minutes of backoff while
  `list_documents` already had the body. Each arm on that route now logs
  the artifact id, the arm, and the underlying error. The GET wire sentence
  is `artifact exists, read failed — retryable. Recover the body with
  list_documents.` A missing well-formed id stays a 404. The 2026-09-22
  22:20Z window is gone from the dogfood container (restarted 23:49Z), so this
  does not guess a cause for that incident. Verified: `bun run verify` and
  `bun run api:check` (fmt, clippy `-D warnings`, cargo test) are green; the
  retry sentence is pinned by
  `transient_read_names_the_recovery_and_hides_the_engine_error`;
  `unknown_well_formed_id_is_404_not_500` against the worktree database
  answered 404 `{"error":"not found"}`.

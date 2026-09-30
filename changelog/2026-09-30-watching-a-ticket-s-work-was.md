- **Watching a ticket's work was forbidden for everyone, always.** The agent
  pane reported "the watch stream is not connected — forbidden" and the Turns
  tab "could not load the harness turns — forbidden", and both were telling
  the truth: `/api/runs/:id/watch`, `/api/runs/:id/transcript` and
  `/api/runs/:id/events` all gate on `may_watch_run`, and for a run whose
  subject is a ticket that gate resolved the ticket's board through a
  `TASK_BOARD_ID` OnceLock **that nothing in the codebase ever set**. An
  unset seam answers `None`, `None` on that edge means the ticket has no
  board, and no board means NotAudience — so every work session ever
  dispatched was unwatchable by every user, including the board's owner.
  Verified against the live instance: the run row is shaped correctly
  (`subject_type=task`, owner null) and `board_role` returns `owner` for the
  person viewing it, so the ACL had every fact it needed and threw them away
  at the missing hop. The seam is now deleted rather than registered: it read
  as a dependency-cycle break but was not — the same module already calls
  `talaria_boards`, `talaria_channels`, `talaria_conversations` and
  `talaria_users` directly on the edges either side of it, and the question
  is one column, so it is now one query. A new `unset-boot-seam` invariant
  makes this class fail `bun run check` instead of production: it finds every
  `pub static X: OnceLock<…dyn Fn…>` that is read and never set, carries the
  fourteen that are dead today as an explicit shrinking census, and fails on
  a fifteenth. That census is itself the finding — this pattern had already
  shipped twice before (`CONVERSATION_OWNER`, "dead in production for as long
  as it did", and `KB_DOC_ALLOWS_READ`, "caught only by review"), and the
  audit found twelve more, including a registry that caches an empty
  Workbench tool catalog and an admin home that always reads zero alerts.
  Verified: `bun run check` clean with the census, and failing as intended
  when a new unset seam is introduced.

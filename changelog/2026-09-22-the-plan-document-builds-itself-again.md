- **The plan document builds itself again: server-side sync replaces the
  client's witnessed-landing guess (TALA-33).** The plan's living document was
  rewritten only when a client POSTed `/api/plans/:id/doc` — and the client
  fired that only when THIS tab watched a turn's in-flight → complete flip.
  Three ordinary gaps silenced it: a reply longer than the chat view's ~4
  minute resume poller, a turn landing behind a queued message (the landing
  check read only the last row's role), and a stream-death retry whose
  completion the tab never saw. The pane then sat stale until someone pressed
  "Sync from chat" by hand. Now the server calls the same `sync_plan_doc`
  rewrite (whole-document contract and data-loss guard intact, `tier` routed
  and metered exactly like the manual route) as a detached task when a plan
  turn persists complete; a per-plan in-flight guard skips overlaps and a
  5-second recency window skips a double-burn behind a manual sync the client
  already fired. The doc pane listens on the conversation event firehose, so
  it refetches and renders the new version even for landings this tab never
  witnessed or that other members' tabs triggered. The chat view's landing
  arm is conversation-scoped (a turn landing behind a queued row now fires
  `onTurnComplete`) and the resume poller lost its tick cap — its lifetime is
  the turn's, not four minutes. "Sync from chat" stays as the explicit
  fallback, and a failed auto-sync logs loudly rather than failing the turn.
  Verified: `cargo test --lib chat_persist` (auto-sync policy suite), clippy
  `-D warnings`, `turn-landing.test.ts` (6 tests) in vitest.

- **`list_boards` no longer 500s for agents — the port's one never-executed
  query.** The agent listing's `select distinct` sorted on `b.updated_at`,
  but the boards port had replaced the raw timestamps with epoch-ms
  expressions in the select list, and Postgres refuses a DISTINCT whose
  ORDER BY expression isn't selected — so the statement was rejected
  outright and every agent-authenticated boards call returned 500 from the
  day the module landed (the fleet's first smoke test found it; the
  user-session listing has no DISTINCT, which is why the UI never showed
  it). The epoch columns now leave as named columns and the sort uses the
  name. A live-DB suite (`api/tests/boards_store.rs`, `#[ignore]`d) now
  executes the listing against the real table so an illegal shape can never
  land again.

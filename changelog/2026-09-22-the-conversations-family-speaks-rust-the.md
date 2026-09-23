- **The conversations family speaks Rust — the durable chat crosses.**
  `/api/chat` (whole-path, the family's only EXACT proxy entry), the
  `/api/conversations` list and `{id}` detail/rename, and `/api/dms` serve
  from Rust. The engine crossed as `chat_persist`: the gateway stream is
  teed through a bounded channel whose receiver is the client's SSE body,
  while the drain that persists history runs detached — an in-progress reply
  survives a client hang-up. The persist half carries the throttle (400ms,
  first event immediate), the token ledger (real usage or a char estimate),
  the confab guard awaited AHEAD of the plan-brain index and the @mention
  mail (strict mode redacts before either takes its copy), the Titler's
  first-exchange rename, and the queued-message continuation chain — a
  message that landed mid-stream starts the next turn, honoring the effort
  stamped on its own row against the model that will actually run it. The
  proxy's response-header allow-list gained `x-conversation-id` and
  `x-message-id`: the SPA learns a new conversation's id from the header.
  Verified by a 69-case byte-diff against TS (reads across chat/plan/
  research access, the dms table, the zod 400 table, a write cycle through
  both runtimes including a queued 202 and a collaborator's rename, and
  the SSE streams byte-identical via a manifest-absent model's canned
  reply), plus a live-agent turn proving stream → persist → ledger with
  real token counts end to end. The diff caught three port bugs: the
  detail select's one nullable column (`guard` — null on the wire, not
  `[]`), the DM's foreign-key 400 carrying sqlx's decorated message where
  TS answers the bare Postgres sentence, and the SSE header order
  (`cache-control` belongs above `content-type`).

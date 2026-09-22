- **The brief family speaks Rust — the assistant's morning document crosses
  with its reader.** Five routes under one proxy prefix: the document read
  (sweep-if-due, then the fold), the line verdict (check/dismiss/restore
  with its three 404 sentences), the read cursor (monotonic — it only ever
  advances), the delegation pair (grant/revoke the reply-without-asking
  privilege, where granting permission for an already-written reply means
  sending it), and the draft verdict — the one route in the family that
  causes something to leave, so it is the one that refuses on staleness
  (409 "They have said something since this was written…" rather than a
  400). The engine's reader plane crossed with them: the absent literal
  `{absent, nextAt, agent}` across all three kinds, the fold as typed
  structs in TS declaration order (a resolved line sinks but is never
  dropped), and the delegation state machine. Verified by a 39-case
  byte-diff against TS on the same sessions — the document read literally
  (all three `tz` variants), every validation shape, the check-off
  idempotence and the restore ladder, the grant→release(sent:1)→revoke
  cycle, the foreign-channel 403, and the reply ladder. The brief is one
  document per person per day, so the harness warms the shared document
  first and then works per-side probe rows and seeded resolved pairs. The
  diff caught four decode bugs on its first run — the same two families as
  before (uuid columns decoded as `Option<String>` need `::text`; INT4 seq
  columns decoded as i64 need `::int8`). The `pending` absent kind is
  unreachable at the harness's hours and is pinned at the engine level
  only.

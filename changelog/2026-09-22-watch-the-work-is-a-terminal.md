- **Watch the work is a terminal now: the agent's own words and tool calls,
  streaming live from the run — on the board card, not buried in the ticket.**
  A work-session turn tees every parsed stream event (prose as it lands,
  tool calls as they start, reasoning, failures) to `run-watch:<id>` with a
  per-turn replay tail; `GET /api/runs/{id}/watch` (run-ACL-gated like the
  run's event stream) answers an SSE of bounded replay then live frames.
  The watch modal renders it as a pinned terminal — tool markers on their
  own lines, flowing prose beneath, previous turn's reply tucked behind a
  disclosure. Board cards with live sessions now wear the dither field
  themselves (a working cell before the columns, one board-wide sessions
  read), with the watch affordance ON THE CARD; the ticket strip keeps its
  live phase. Turn counters show `turn N` — no invented "/12" denominator —
  and the modal never skeleton-locks: the terminal opens with whatever the
  turn has produced so far, or says it is waiting for the next output. One
  new door verb, `getStream` (the GET twin of `postStream`), carries the
  client side through the one-HTTP-door invariant (count 7).

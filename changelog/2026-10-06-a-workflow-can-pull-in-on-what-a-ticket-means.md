- **A task workflow can now pull in on what a ticket means, not just on a
  substring.** `match_workflow` is case-folded containment over a
  `title\ndescription` haystack, so a workflow keyed on `"delivery failure"`
  does not fire on a ticket that says *"the webhook retries are flaking"* — and
  nobody finds out. The hook silently does not fire, the agent works the ticket
  without the skills and toolkits it was supposed to get, and the output is
  merely **worse** rather than wrong, which is the hardest kind of miss to
  notice.

  A second pass asks a decision model, one yes/no per workflow over the ticket,
  all in a single request. It runs **only over the workflows the keyword match
  missed**, and it can only **add** to the delivery — a keyword match is never
  dropped, so the worst case is an agent handed a skill it did not need rather
  than one missing a skill it did. With the port off, which is the default, the
  answer is byte-for-byte what `workflows_from` already returned.

  **The threshold is high (0.75) and that is a different judgement from the
  ticket gate's.** The gate leans toward yes because a missed instruction costs
  more than an unneeded reply. Here there is no such asymmetry: a workflow
  carries skills and toolkits into an agent's session, so a weak yes is not
  worth acting on. Every judgment is recorded to the shadow ledger under
  `workflow-match` — including the ones below the floor, because an agreement
  rate computed over only the acted-on half is not one — so the number can be
  chosen from observed traffic rather than left at a guess.

  A hook with neither a name nor a description is **skipped** rather than asked
  about: a yes/no over an empty string is a coin flip dressed as a decision.

  **Both call sites moved**, including the one that matters most. The dispatch
  path reaches workflows through a `WorkflowsFn` that hands a `PgPool` — enough
  for a substring match, not enough to ask a model — so the real deps closure
  now captures the same `AppState` the struct beside it already holds. Widening
  the fn type would have touched every fake in the work-session tests for
  nothing.

  One correction to the plan this came from: it predicted the question
  construction would have to live in `talaria-workflows` with the call made by
  the two sites that hold state, because the crate is a leaf. It turns out
  nothing depends on `talaria-workflows`, so it can take `talaria-decide`
  directly and own the whole pass — simpler than the split, and the call sites
  change by one identifier each.

  Verified: 10 tests in `talaria-workflows` (4 new — a hook with nothing to
  judge is skipped, a judgeable one leads with its name, the subject carries the
  tags because a near-miss label is exactly what is being judged, and the gate is
  pinned by BEHAVIOUR at its boundaries — `acts_on(Some(0.75), true)` acts,
  `0.74` does not, and an uncalibrated `0.99` never does. Clippy is what forced
  that shape: the first draft asserted the constant against itself, which is a
  compile-time tautology and rightly rejected under `-D warnings`). clippy clean
  across
  `talaria-workflows`, `talaria-runs-work-session` and `talaria-routes-boards`;
  `bun run gate` green. Not yet exercised against a live decision provider: the
  pass is covered by its pure tests and the port's own, but no real round trip
  has judged a real ticket.

- **Three dead cross-crate seams, one 500 that should have been a 400, and two
  test contracts the first live-suite runs put right.** The new live-DB gate
  (below) ran for the first time and everything it found is fixed in the same
  breath — no exclusions added:
  - `CONVERSATION_OWNER` (talaria-attribution): the seam and its resolver —
    `conversations::conversation_owner`, whose doc comment names the
    attribution ladder — both existed, but nothing ever called `.set()`, so
    the ladder's live-turn rung silently fell through to the hirer on every
    install: an org agent creating mid-turn attributed its output to whoever
    hired it, not the human it was answering.
  - `GET_TASK` ×2 (talaria-workchains, talaria-inbox-focus): same disease,
    sharper teeth — those sites `.expect()`ed the unset seam, so the
    workchain turn/pause notification paths PANICKED in production (caught by
    catch-panic as opaque 500s) instead of doing their work. Both wired, and
    no armed panic paths remain: the workchains sites skip with a warning,
    and the inbox-focus site throws a failure its decision machinery already
    records on the card (the message is what the user reads).
  - The step-wedge insert (workchains_id.rs): `after` naming a task outside
    the chain put a NULL-position row through the insert's SELECT and died on
    the NOT NULL constraint as a 500 — the designed 400 ("rows_affected 0 =
    the anchor is not in this chain") was unreachable. The insert now selects
    zero rows for a missing anchor and the 400 fires as designed.
  - Test contracts: workchains' heartbeat used agent_defs.id where the route
    resolves fleet_agents.id (404 "unknown agent"); the pause assert raced the
    detached engine instead of polling like its sibling helpers; realtime_fan
    asserted member order that only holds when uuid order coincides with
    insertion order — a lottery, now sorted on both sides.
  All three seams are set in `register_all` (the composition root) and
  mirrored in `tests/support::wire_boot_seams`, which test binaries call
  because they never boot the scheduler.
  Verified: attribution (6), workchains_live (10) and realtime_fan (1) green
  against scratch postgres:16 + redis:7 containers in this worktree — each
  red before its fix, green after; the FULL ignored suite dress-rehearsed
  locally the same way, zero exclusions beyond the two container-impossible
  binaries.


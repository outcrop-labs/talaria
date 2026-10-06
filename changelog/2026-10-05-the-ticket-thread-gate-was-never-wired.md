- **The ticket-thread gate had been dark for sixteen days, and two gates now
  catch the class.** `ticket_message_relevant` is the only door between a human
  message on a ticket's discussion and the assigned agent's turn. It settles the
  structural cases itself — an attachments-only handoff, a bare empty turn, a
  message naming the ticket ref — and hands everything else to the
  `ticket-relevance` harness through a `OnceLock` edge. **Nothing set that
  edge.** The `None` arm answers `true`, so every remaining message read as the
  agent's business: the gate was not degraded, it was absent, and the assigned
  agent replied to people talking to each other about Thursday's ops review —
  precisely the "roommate" failure the harness's own header was written to
  prevent.

  How it went dark is the part worth recognising. `82eb786e` ("api: extract
  ticket-thread model surfaces") moved the module into its own crate and
  introduced the `OnceLock` to break the dependency on the harness runner —
  correctly — but never added the setter. Its verification line reads
  "Verified: cargo check -p talaria-api", and `cargo check` cannot see an unset
  `OnceLock`: the declaration compiles, the read compiles, and the fallback is a
  valid answer. The live instance's `harness_runs` dates it exactly — nine
  `ticket-relevance` rows between 2026-09-18 02:37 and 2026-09-19 00:28 UTC, the
  extraction landing at 04:28 that morning, and not one row since.

  The fix is one `set` in `register_all`, restoring the pre-extraction fold
  verbatim: a harness error, a null verdict, or any value that is not
  `{"relevant": bool}` all answer `true`. The gate may cost an unneeded reply;
  it may never cost an unanswered one.

  **Two new gates, because this is the fourth time.** `register_all`'s own
  comments record three others found the hard way (a five-minute delay on every
  queued Google write, a 500 on every plan-draft POST, an attribution ladder
  crediting the wrong person). So: a `oncelock-seam-never-set` invariant that
  fails when any injected edge is read but never set anywhere in the tree, and a
  boot assertion in the jobs completeness test. The invariant carries an
  `UNSET_SEAM_CENSUS` of the **12 other edges that are currently never set** — a
  backlog, not an exemption list, and one that can only shrink: a companion
  check fails if a listed seam gets wired or stops existing. Those twelve are
  *unaudited*; an unset edge kills the call path that reads it, not necessarily
  a whole feature (the Titler has 49 live harness runs despite one of its edges
  being dark), so each needs its own look.

  Verified: both new gates proven to fire against the real bug, not just to pass
  with it fixed — deleting the setter makes `bun run check` fail naming
  `api/crates/talaria-ticket-chat/src/lib.rs:28`, and makes
  `register_all_declares_the_whole_ported_table` panic with the gate's own
  sentence; restoring it makes both green. Both census-stale branches tested the
  same way (a wired seam and a nonexistent one each fail with their own
  message). `cargo test -p talaria-jobs` 2 passed; `cargo clippy -p
  talaria-jobs` clean; `bun run check` clean (27 `OnceLock` seams declared, 12
  unset, matching the census). The dark window itself was established from the
  live instance's `harness_runs` table, not inferred from the code alone.

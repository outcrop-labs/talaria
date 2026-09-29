- **A ticket's run history and logs are readable again — live, and after the
  run ends.** Three independent faults had to line up, and all three were
  live. The watch stream was dead on arrival: `/api/runs/:id/watch` built its
  subscriber with `RealtimeDeps::publish_only`, whose subscribe edge is
  `quiet_subscribe()` by construction — its own doc says nothing on that plane
  will ever open a stream — so `recv()` resolved to `None` immediately, the
  body ended the moment the replay ran out, and the pane showed a tail of
  history and then sat on "waiting for the agent's next output" however much
  the agent went on to say. It now uses `streams_only`, the same constructor
  the `runs_events` sibling already used. Turn transcripts were written with
  no visibility, so they took the `artifacts.visibility` column default of
  `private` while being ownerless — an agent wrote them, so `created_by` is
  the agent model and `owner_user_id` is null — which made the ticket's own
  review record unreadable by every human who opened the Turns tab. They are
  org-visible now, set on every save so rows written before this heal on the
  next turn. And the modal destroyed the record exactly when it became worth
  reading: every call site passed `onEnded` as "close", and the ticket strip
  additionally nested the modal inside its `{#if live}` guard, so the pane
  vanished the instant the session dropped. `onEnded` is now an optional
  notification that no caller closes on, the strip latches the run id so the
  modal outlives the session, and closing is a person's decision. A run
  ending also refreshes the artifacts behind the Turns tab, which is when the
  last turn's transcript lands. A watch stream that cannot open now says so
  in red instead of rendering as a quiet agent — a working stream and a broken
  one looked identical, which is why this went unnoticed. Verified:
  `bun run check` and `bun run typecheck` (5413 files, 0 errors). The Rust
  half was NOT compiled locally — three worktrees were already cold-building
  on this box at load 40 — so `fmt + clippy + test (api)` in CI is the proof
  line for `runs_watch.rs` and the transcript visibility, not a local run.

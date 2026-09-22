- **The dead TS engines go too — `ui/src/server` is down to its live tier.**
  The cutover deleted the routes; this strip deletes what they left behind:
  ~106 files (54 engines + their tests) removed by a reachability audit from
  the live seeds (server-entry, `app.ts`, the four residents, the SDK, the
  non-server `ui/src` tree, the CLI and mcp packages) — the boards/tasks/
  statuses/approvals/notifications engines, the scheduler and its lease,
  inbox-focus, research/outreach/work-dispatch/briefing, the Google engines,
  fleet render/reconcile/preflight/brain/crons, storage/uploads, retrieval,
  channels/comms, daily-brief, muse route halves, and more. Two near-misses
  the audit caught before they shipped: `app.ts` itself (the SPA's SSR entry —
  vite dev-middleware, the server build, and boot's `migrate` hook all reach
  through it) was restored, and `scripts/gen-github-jwt-vectors.mjs` (wired
  into `api:vectors`) imported the deleted signer, so it was deleted and the
  fixture it generated is now frozen ground truth for
  `api/tests/github_jwt.rs`. The TS updater's scheduled check went with the
  scheduler: the job never registers, the manual check/apply halves and the
  admin panel stay, and the hold is documented at both ends (the Rust api
  cannot rebuild and restart its own binary — see `api/src/jobs.rs`). Two
  invariant rules retired with the surfaces they watched (the
  hand-written-harness census and the toolbox anchor re-pointed at the Rust
  fitness toolbox); a new cross-language pin test reads
  `OFF_BOARD_STATUSES` out of `api/src/statuses.rs` and fails if the TS
  client's copy ever drifts (fail-closed: an unparseable literal parses to
  an empty list and fails). The surviving ~54-file tier is everything the
  residents, the SDK, and the SPA shell actually import — plus the comment
  and doc citations across the tree re-pointed at their Rust twins.

### Changed

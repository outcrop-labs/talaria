- **Workchains: the handoff engine and the workchain-aware agent
  heartbeat.** A ticket landing in a done column now advances its chain:
  the new head's human assignees get an in-app `workchain_turn`
  notification ("It's your turn: <ref> - <title>", deep link
  `/boards/{board}/{task}`); an agent head hears nothing — its visibility is
  the heartbeat serving it. A ticket landing off-board (failed/cancelled)
  pauses the chain and notifies the chain's creator
  (`workchain_paused`, "Workchain paused: <chain> - <ref> <title>");
  unpausing is a human PATCH that re-derives the head, advancing nothing.
  The engine derives from the DB at write time — no in-memory chain state —
  and fires only on the normal status-write paths (`update_task` and the
  review sign-off in `complete_quality_review`), where human sign-off is
  the only trigger by construction (agents cannot land terminal columns).
  The agent heartbeat now enforces the chain's ordering: a step behind an
  earlier live step is excluded from `work_items` whatever its own column
  says, and a ready head carries `workchainReady: true` (absent on
  chain-free tickets — byte-identical feed shape for them). Verified:
  `bun run check` green; `cargo fmt --check` green; `cargo check --lib`
  green (the full clippy and test-target builds are SIGKILLed by the box's
  4 GiB cgroup — six attempts, every one an OOM kill with zero lint or
  compile findings surfaced; they must run in CI). The
  `derive_states`/`terminal_of` unit tests and the extended
  `workchains_live` router suite (4 more `#[ignore]`d tests: human-head
  turn notification, heartbeat ordering + ready flag + all-done,
  failed → pause + creator notification + unpause semantics, archived
  mid-chain head) need a dev Postgres + Redis to run.

### Added

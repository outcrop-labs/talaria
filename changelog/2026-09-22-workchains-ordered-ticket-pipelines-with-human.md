- **Workchains: ordered ticket pipelines with human/agent handoffs (API +
  data model).** Two new tables — `task_workchains` (a named, position-
  ordered chain on a board) and `task_workchain_steps` (the chain's tickets
  in position order, unique per task across all chains) — land in the
  migrations array, with the CRUD API: `GET/POST /api/boards/{id}/workchains`
  (list with derived per-step state — done/head/waiting/archived, computed
  from the board's real status categories; create), `PATCH/DELETE
  /api/workchains/{id}` (rename, pause, reorder `positions: [{taskId,
  position}]` — all-or-nothing, a miss refuses the whole write; deleting a
  chain unlinks, it never deletes tickets), and `POST /api/workchains/{id}/
  steps` + `DELETE /api/workchains/{id}/steps/{taskId}` (add after an
  optional step, remove without touching the task). One chain per ticket is
  a v1 invariant backed by a unique index; cross-board adds answer 400,
  duplicates 409, unknown chains 403 like the boards family. Access mirrors
  board configuration: any member reads, owner/editor writes; every write
  bumps the board's SSE stream. Verified: `bun run check`; `cargo fmt`;
  `cargo check --lib`. Clippy and the full test pass are still owed — the
  box's 4 GiB cgroup, shared with sibling agent sessions, cannot fit the
  compile — as are the live proofs: `derive_states` unit tests (4,
  in-module) and the `workchains_live` router suite (6 `#[ignore]`d
  tests: create/list-with-derived-states, one-chain-per-task incl. the
  unique-index refusal, task-delete cascade, cross-board refusal,
  reorder + step-delete order honesty, chain-delete leaves tickets
  standing) need a dev Postgres + Redis.

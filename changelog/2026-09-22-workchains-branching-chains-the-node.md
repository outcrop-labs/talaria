- **Workchains: branching chains, ports, and wires — the node canvas.**
  The model is edges, not an ordered list: `task_workchain_edges`
  (from_step → to_step) turns a workchain into a DAG the api keeps acyclic
  on write — a wire that would let a step reach itself is refused with a
  400 before any write. The migration derives every v1 chain's edges from
  its linear order, so existing chains keep working unchanged; the
  `positions` PATCH verb still speaks linear and rebuilds one line over
  the edges. Derived state stays derived, now from predecessors: a step
  with no predecessors is a head, one whose predecessors are all
  done/archived is ready (the AND-join — a step with several predecessors
  waits for the last of them), the rest are blocked; a failed predecessor
  is not satisfaction, the chain has paused instead. The handoff engine
  fans out: a step landing in a done column makes EVERY satisfied
  successor ready at once, each telling its human assignees it is their
  turn (`workchain_turn`); agents hear nothing — the heartbeat serves
  ready heads, several at a time now, and hides blocked steps as before.
  An off-board terminal still pauses the whole chain and notifies the
  creator (v1 semantics; whether a failure should quarantine only its
  downstream subtree stays an open triage question and nothing stored
  depends on the answer). A chain that BRANCHES renders as the node
  canvas instead of the rail: free-placed cards with a visible in-port and
  out-port, wires as cubic beziers whose state mirrors the derive (idle
  gray, fired accent with the dash running, done success), assignee
  avatars so the human/agent mix reads at a glance; cards drag and their
  position persists (`canvas_x`/`canvas_y`, null = auto-layout by
  longest-path levels); clicking a wire cuts it (`DELETE
  /api/workchains/{id}/edges/{from}/{to}`, POST draws). Linear chains keep
  the rails. A step's assignee is still the ticket's assignee — no new
  agent-writable step-assignee surface, and the drag-to-connect editor
  itself is TALA-34's, built on these ports. Verified: `bun run check`
  green (after regenerating the API reference — which required fixing the
  docs generator's per-module grouping, see its own entry);
  `workchains.test.ts` 29/29 (fan-out, AND-join, failed-pred, diamond,
  cycle-direction cases); svelte-check 0 errors; `cargo fmt --check` and
  `cargo check --lib` green. The clippy/test-target builds are SIGKILLed
  by this box's 4 GiB cgroup (the v1 engine entry's same finding) and the
  live router suite needs dev Postgres + Redis, absent here — both must
  run in CI.

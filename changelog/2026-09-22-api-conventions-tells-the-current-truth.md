- **API-CONVENTIONS tells the current truth.** The page still described a TS
  route tier the cutover deleted: the reference "frozen until #293" (the
  extractor landed — it is generated, and drift fails check), a TS twin of the
  agent-ticket predicate in `server/tasks.ts` (the file is gone; the predicate
  lives once, in `api/src/tasks.rs` — `agent_safe_patch` through `update_task`,
  the exported `agent_ticket_refusal` for every door that never reaches the
  patch), an invariants table naming rules that left with those routes
  (rewritten against the live script: swallowed query errors, flattened query
  results, dropped `listQuery` notices, hand-rolled popover engines, and the
  off-board / board-policy / fetch-door censuses), `.tsx` board filenames
  (`Kanban.svelte` and `BoardList.svelte` refuse a failed statuses read,
  `Gantt.svelte` marks it inline; the dead ci.yml note is gone), and an
  audit-task pointer that resolves nowhere. Every claim in the rewrite was
  re-verified against the code it names.

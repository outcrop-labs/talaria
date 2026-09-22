- **Workchains: the ticket's own chain section.** A ticket in a chain
  shows a Workchain block in its detail view — chain name, `step N of
  M`, and (for editors) move-earlier/move-later/leave verbs. The move
  ships the full reorder payload (`moveStepOrder`, unit-tested: swaps,
  end refusals, unknown ids, no caller-mutation); leaving unlinks the
  ticket and the chain reads on. Joining stays in the Workchains view's
  pickers; step-assignee edits are deliberately absent (a step's
  assignee is the ticket's assignee — one edit surface, and the MCP
  guardrails hold by construction: no agent tool reaches the
  workchain routes). Verified: `bun run verify` green (1179 tests);
  the section driven in headless Chrome — renders for a chained
  ticket, correct position read (screenshot filed with the ticket).

- **Agent refines announce themselves: the silent document swap is gone.**
  When an agent edits an open document through its toolkit
  (`edit_kb_doc`, `update_document`), the surface now says so — an
  "Agent refine" notice on the KB doc and artifact views (read and edit
  panes alike) naming who wrote the revision, when, and a change summary
  (±lines), with a Show-changes diff against what the viewer held when the
  refine landed, a Load-into-editor review path, and a per-viewer dismiss
  that persists across remounts (localStorage, keyed by viewer identity).
  The updated body appears without a manual reload: read panes follow the
  server on announce; the editor's buffer is never yanked — the refine
  stages behind the notice and lands only when the viewer loads it. A
  viewer's own saves are never announced, detection polls `/api/history`
  every 4s, and the notice announces the newest unseen revision only —
  one refine at a time, the same way a reviewer reads. Decisions live in
  `ui/src/lib/agent-refine` (unit-tested, 17 cases), the DOM in
  `AgentRefineNotice.svelte`. The Muse accept stopped being a vanishing
  act too: it folds with a ±line receipt, and the Draft/Refine button
  carries a pulsing in-progress state while streaming. Muse failures keep
  their error line. Verified in a real browser against an API stub: the
  notice announces, the diff opens with the refined text, load stages and
  saves, read panes refresh without a reload, the receipt shows the
  accept's change, a failed Muse run shows the error, and the artifact
  surface announces the same way (screenshots in the ticket).

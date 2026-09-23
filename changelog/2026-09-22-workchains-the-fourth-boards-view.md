- **Workchains: the fourth Boards view.** The view toggle gains a
  `workchains` lens (`?view=workchains`, URL-driven like the others,
  saveable as a saved view, persisted per board in localStorage). Each
  workchain renders as a horizontal rail of compact step cards — status
  dot, ref, title, effort, due, assignee avatars — joined by chevrons:
  done steps fade under a check, the head wears the accent ring and a
  `HEAD` marker, waiting stays quiet, archived steps strike through and
  always render (they are chain structure, not filter fodder). Below the
  rails, UNCHAINED lists the tickets no chain holds, each row offering a
  `+ chain…` picker; it collapses once chains exist. Rails read the
  board's filtered task set (a step whose ticket is filtered out is
  hidden, chevrons reconnect what remains) and stay live through the
  board's SSE stream (`useBoardLive` invalidates `['board-workchains']`
  alongside the cards). The add-ticket picker rides the §7 Popover shell
  with search; the step-card summaries render off the chain read rather
  than the pills (which want a full Task). The skeleton grows a matching
  rail-shaped lens so the canvas shifts rail count, not widths. Wire
  types are the strict ones (`Effort`/`TaskStatus`); the pure rules
  (`buildPositions`, `chainProgress`, `chainedTaskIds`) live in
  `workchain-rules.ts` with the 9 unit tests, split from the
  svelte-query client (`workchain-client.ts`) so the node suite can run
  them. Verified: `bun run verify` green end to end (check + svelte-check
  0 errors + 1175 tests across 68 files), and the view driven in a real
  headless Chrome against a stubbed API — rails render with derived
  states (`1/4` progress, `HEAD` ring on the in-flight step), the lens
  round-trips through the URL against board/list/gantt, and clicking a
  card opens the ticket overlay (screenshots filed with the ticket).
  Live-data pass and the interactive writes (add step, reorder,
  pause/unpause) still need a dev stack.

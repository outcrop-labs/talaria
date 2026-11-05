- **The knowledge sidebar tree now remembers which folders you had open.**
  Doc rows and KB-space sections start collapsed — the tree is deep, and
  "everything visible" was never the shape you left it in — and expansion is
  remembered across navigation: leave to a board and come back, switch spaces
  and switch back, and every branch you opened is still open, every branch you
  closed still closed. KB-space sections get their own collapse chevron, so
  the sidebar folds up one more level; clicking a space's name still selects
  the space and now also opens its doc list, the way it always felt like it
  should. Creating a nested doc or dropping one inside a folder opens that
  folder, and a remembered document is visible in the tree when you come back
  to it — no more landing on a selection hidden behind a collapsed branch.

  Verified: `bun run test src/lib/knowledge-tree-state.test.ts` (9/9),
  `bun run typecheck` in `ui/` (5732 files, 0 errors), `bun run check` at the
  repo root — all green.
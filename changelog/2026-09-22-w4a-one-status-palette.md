- **W4a — one status palette.** `ui/src/lib/statuses.ts` owns `STATUS_COLOR`
  (8 keys); `components/board/kanban.ts`'s `COL_ACCENT` and
  `components/board/field-pills.ts`'s duplicate are deleted, and the three leaf
  call sites that imported them (`Kanban.svelte`, `StatusPill.svelte`,
  `FilterBar.svelte`) read the shared map, each keeping its own unknown-key
  guard. **One rendered colour changes, measured rather than assumed**:
  `COL_ACCENT` carried only the six on-board keys, so Kanban's legacy off-board
  lane renders `failed` as `danger` now instead of `muted` (1 of 16 site×key
  pairs; `cancelled` unchanged, the other 14 unchanged). That is the drift this
  wave removes — every other lens already drew `failed` as danger.

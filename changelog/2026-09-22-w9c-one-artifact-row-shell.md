- **W9c — one artifact row shell.** `RowInteraction` (17 fields) moves to
  `routes/app/artifacts.ts` and the frame to `ArtifactRowShell.svelte`; the row and
  the tile keep their own bodies, markup and classes. Verified by rendering new
  vs old across 42 combinations (3 rows × 7 interaction states × 2 views) and
  diffing the HTML — identical modulo attribute/class order.

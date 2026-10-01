- **TALA-11 (part 1): the hire now happens on a full-panel splash.** While the
  AI designs a new agent, the describe step's body swaps to a tall dithered
  panel (`GeneratingSplash`) instead of a small inline generating block — the
  same moment at the scale of the thing being made. Same material (edge lull +
  two crossing waves, retuned so the splash reads as its own moment, not an
  enlarged copy of the block), same honesty rules (no percentage, nothing that
  sweeps toward an end; the elapsed seconds stay silent until 10s and then
  speak plainly). The footer row — Designing button with its waiting mark,
  Cancel — stays reachable for the whole wait. The refine path on the review
  step is untouched; this is the initial-prompt generation only.

  Verified: new colocated unit test (`generating-splash.test.ts`, 6 cases
  across the label rule and the field's shape) passes; `bun run typecheck`
  (svelte-check, 5422 files) clean.

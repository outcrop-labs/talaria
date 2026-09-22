- **The dither field repaints what changed, not the whole field — the desktop
  shell's launcher stops being a furnace.** `ui/src/lib/dither-engine.ts`'s frame
  re-evaluated and re-drew EVERY cell of every mount, every tick. At the house
  grain (pitch 2 / dot 1) the launcher's full-window field is 247,040 cells, and
  keeping it shimmering cost 575ms per paint on average (633ms worst): the page
  ran at 1.5 fps with 84% of the main thread inside long tasks — and the launcher
  stays mounted (and painting) behind an active instance. The field is a function
  of the sources' GEOMETRY, so it is now computed once per change
  (`density`/`ink`, per cell) and a shimmer tick re-rolls only what the shimmer
  can change: the threshold, for the cells whose lit state can flip
  (`|density − threshold| ≤ shimmer/2`). Every other cell is left as it is, cell
  by cell, against a `painted` buffer in the canvas's own 8-bit resolution, and a
  colour string is built once per distinct pixel instead of once per cell. A LIVE
  frame (a tween in flight, a travelling wave) still re-evaluates and redraws, so
  transitions keep full rate. Two deliberate consequences: the ALPHA no longer
  rides the shimmer jitter (it is a function of density alone — a jitter that
  moved every cell's alpha by a level made every pixel differ on every tick; the
  visible sparkle, the lit/unlit step, is unchanged), and a field nobody can see
  stops painting — an IntersectionObserver on the canvas plus `visibilitychange`
  park the loop, and becoming visible repaints from a clean canvas.

  Measured in a worktree stack (its own DB/Redis, app :5302; the launcher page on
  :5290), same method before → after: the launcher field 575ms → 7ms per paint
  (633ms → 17ms worst), 13 long tasks and 7.6s of blocking in 8s → 0 and 0, 1.5 →
  60 fps, main thread 100% → 7.5% busy; the inbox's full-pane empty-state field
  85ms → 2ms per paint, 50 long tasks and 4.6s → 0, 34 → 60 fps, 62% → 5.6%.
  Parity proved rather than assumed: the new paint path rendered against the
  previous loop verbatim into a second canvas differs in **0 pixels** for three
  source mixes (edges+organic, rect+halo+ramp, cover mode) and for a masked
  field; the shimmer rule was checked over 525,525 (density, threshold, jitter)
  triples with **0** cases where the lit/unlit decision differs from the
  density-jitter it replaces. Five routes (inbox, brief, boards, knowledge,
  artifacts) load with no console errors, fields painted, 0 long tasks after
  settle. The desktop GUI itself was not run — this host has no display — so the
  launcher was exercised as the page it is, in a browser. `bun run check` and
  `bun run verify` green.

### Sweep waves W4-remainder, W8-W12 — landed in parallel, one slice per owner
Each slice below is an independent commit's worth of work; all of them are in the
tree together and share one validation pass.

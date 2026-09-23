- **The dither field is painted in absolute pixels again — an incremental
  repaint was compositing over itself, and every shimmer field crept past its
  own maxAlpha into blotch.** The incremental-paint engine (#410) replaced the
  full clear-and-redraw with a per-cell diff, but `fillRect` composites
  source-over: a dimmer refill over a brighter dot BRIGHTENS it (0.016 over
  0.063 reads 0.078), so every shimmer flip and every cell a passing wave crest
  dimmed stacked another layer — the rail's hover whisper rendered at ~3× its
  alpha with pixels above the field's ceiling, the brief hero and the border
  fields accumulated the same way, and it worsened the longer a field lived.
  Changed cells are now cleared before they are filled, which makes each write
  the same absolute pixels the full repaint always left; and the shimmer
  jitter rides the density again (both the alpha wobble and the lit/unlit
  step — the field breathes as one material) instead of flipping isolated
  threshold-edge cells. The cached-field plumbing is untouched: density is
  read, not re-evaluated, per tick, and static fields still park their loop.
  Verified: old and fixed engines rendered side by side on the rail's exact
  ambient field are pixel-identical at rest and at settled hover (4 differing
  pixels of 676k; inked 10,020 and meanAlpha 0.0492 on both), a travelling
  wave soaks 6.6s on both with identical stats and no creep (max alpha 130 of
  a 140 ceiling on both), and in the running app the rail reads 7,584 inked
  cells at rest and ~11,000 hovered with nothing above the field's maxAlpha.
  `bun run verify` green (svelte-check 0 errors, 1,186 tests).

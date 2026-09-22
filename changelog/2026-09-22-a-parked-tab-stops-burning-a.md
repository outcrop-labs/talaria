- **A parked tab stops burning a core — the standing wave fields now paint on
  a bucketed clock.** A dither wave field animates forever by design, and the
  engine repainted at full requestAnimationFrame for it: the daily brief's
  hero (always mounted on Home) alone measured **47.5 seconds of JavaScript
  per minute — ~80% of a core — on an otherwise idle tab**, which is the
  "browser slows to a crawl after Talaria is open for a while" report (memory
  stayed flat; it read as a leak but was continuous canvas paint). Waves now
  repaint at ~12/s on the same bucketed clock shimmer already used — a wave
  moves at single-digit pixels per second, a sub-pixel step per bucket, so
  the motion is visually identical — and tweens keep full rate, because they
  are short and they are the transition itself. Verified with a CDP profiler
  against the live instance: heap/DOM/listeners flat over navigation laps
  (no memory leak), and the burn attributed ~70% of samples to the dither
  engine's paint before the change.

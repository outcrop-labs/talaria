import type { DitherSource } from '@/lib/dither'

/**
 * The logic half of `GeneratingSplash.svelte` — kept here because the unit
 * harness is pure node (no DOM, no component mounting), which is how every
 * colocated `*.test.ts` in this tree works: the behaviour lives in a sibling
 * module the test can import (see `inbox-chat-panel.ts` beside its panel).
 *
 * Two things are here because they are the CONTRACT of the splash:
 *   • when the elapsed count is honest to show, and in what shape
 *   • the shape of the ambient field itself
 */

/** Elapsed seconds must genuinely accrue before they are said out loud.
 *  Under this the turn is unremarkable; counting from zero reads as urgency
 *  about a normal wait (same threshold the describe step has always used). */
export const SPLASH_ELAPSED_AFTER = 10

/**
 * The splash's status line: the fixed label, plus the elapsed count once it
 * crosses the threshold.
 *
 * Honesty rules (unchanged from the inline block this replaces): no
 * percentage, no sweep that promises an end. Drift says working; it never
 * says how far along. The count is what separates "working, slowly" from
 * "wedged" — the only number a surface that cannot know the length is
 * allowed to report is the one it can actually measure.
 */
export function splashLabel(base: string, seconds: number): string {
  return seconds > SPLASH_ELAPSED_AFTER ? `${base} — ${seconds}s` : base
}

/**
 * AMBIENT ACTIVITY ACROSS THE SPLASH — the same material as the inline
 * `Generating` block (a lull at the top edge, two crests drifting against
 * each other) but tuned to read as its own moment rather than an enlarged
 * copy of the block: the lull runs deeper and quieter, the long crest is
 * longer and slower, and the crossing crest rides the OTHER axis — the
 * block's two waves both travel along x, so the splash's vertical crossing
 * is the one thing that cannot be mistaken for it.
 *
 * Waves and not a fill for the same reason as there: the design's length is
 * unknown, so any shape that sweeps toward an end implies a completion the
 * surface cannot promise.
 *
 * Reduced motion is the engine's, not this list's: `DitherLayer` subscribes
 * via `onReducedMotion` and `DitherEngine` stops driving waves and shimmer
 * under it, leaving the field textured and still. Verified there; do not
 * duplicate it here.
 */
export const splashSources: DitherSource[] = [
  { id: 'splash-lull', kind: 'edge', side: 'top', depth: 46, strength: 0.14 },
  { id: 'splash-drift-a', kind: 'wave', axis: 'x', wavelength: 240, speed: 20, strength: 0.42 },
  { id: 'splash-drift-b', kind: 'wave', axis: 'y', wavelength: 90, speed: -11, strength: 0.24 },
]

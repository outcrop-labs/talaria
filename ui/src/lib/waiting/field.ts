/**
 * What both renderers share: the dot field.
 *
 * An animation here is a pure function of position and loop phase returning an
 * intensity. Nothing in this file is stateful or time-aware — the frame loop
 * that drives them is `subscribeToClock` in `@/lib/motion`, which is shared
 * with the skeleton field and does not belong to this subsystem. What differs between the two families is only what happens to
 * that number afterwards:
 *
 *   braille.ts   thresholds it   → a dot is on or off  → a text glyph
 *   dot-grid.ts  keeps it        → a dot has an opacity → a grid of elements
 *
 * Braille cannot express the second one — a character is a single glyph in a
 * single colour, so the eight dots inside it cannot have different opacities.
 * That is the whole reason there are two renderers rather than one.
 */

/** Fractional part, correct for negatives — frac(-0.2) is 0.8, not -0.2. */
export const frac = (v: number): number => v - Math.floor(v)

export const clamp01 = (v: number): number => (v < 0 ? 0 : v > 1 ? 1 : v)

/** Shortest signed distance between two angles, in radians. */
export const angleGap = (a: number, b: number): number => {
  const d = Math.abs(((a - b) % (Math.PI * 2)) + Math.PI * 2) % (Math.PI * 2)
  return d > Math.PI ? Math.PI * 2 - d : d
}

/**
 * Deterministic value noise in [0, 1).
 *
 * Stateless BY DESIGN, though not for the reason the playground had: this app
 * is an SPA, so there is no hydration to mismatch. What needs it here is that
 * every field is called at ARBITRARY phases, not just forward in time — the
 * reduced-motion still-frame scans 32 samples looking for the fullest one, and
 * the tests assert on specific phases. A Math.random() would make dissolve and
 * sparkle un-samplable: the still frame would be a different picture each time
 * it was computed, and the animation would boil rather than crossfade.
 *
 * NOTE: `lib/dither.ts` exports its own `hash01` with a different mix. They are
 * deliberately not shared — unifying them would change the output of two
 * unrelated visual systems to save nine lines.
 */
export function hash01(a: number, b: number, c = 0): number {
  let h = Math.imul(a ^ 0x9e3779b9, 0x85ebca6b)
  h = Math.imul(h ^ (h >>> 13) ^ Math.imul(b + 0x165667b1, 0xc2b2ae35), 0x27d4eb2f)
  h = Math.imul(h ^ (h >>> 16) ^ Math.imul(c + 0x94d049bb, 0x9e3779b1), 0x85ebca6b)
  return ((h ^ (h >>> 15)) >>> 0) / 4294967296
}

/**
 * Dot intensity at (x, y) for loop phase `p` (0–1).
 *
 * `w` and `h` are the display dimensions, passed in so a field can be written
 * resolution-independently and reused at another size.
 */
export type DotField = (x: number, y: number, p: number, w: number, h: number) => number

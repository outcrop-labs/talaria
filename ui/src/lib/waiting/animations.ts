import { clamp01, frac, hash01, type DotField } from './field'

/**
 * Twenty-one waiting states for the gap between "send" and the first token.
 *
 * Each one is a pure function of loop phase rather than a frame table, which is
 * what makes them tunable: a wave's wavelength is a number in one place, not
 * ten hand-drawn glyphs to redraw. It also guarantees they LOOP — a field
 * expressed in sin(2πp) has no seam, and a seam is the thing the eye catches
 * first in an indicator that runs for thirty seconds.
 *
 * Periods sit on Mercury's timing ladder (spec §9) wherever the motion allows:
 *   immediate 750ms · standard 1200ms · ambient 2100ms
 * A few need a longer arc to read at all (a heartbeat at 1.2s is tachycardia);
 * those use whole multiples of a rung so they still beat against the rest.
 */

export const IMMEDIATE = 750
export const STANDARD = 1200
export const AMBIENT = 2100

export type Origin = 'reference' | 'original'

export interface WaitingAnimation {
  id: string
  name: string
  /** Honest attribution: from the reference video, or new here. */
  origin: Origin
  /** What it says to someone watching it — the reason to pick this one. */
  blurb: string
  cols: number
  period: number
  field: DotField
  /** Optional per-character opacity. Only the trail-based ones use it. */
  cellAlpha?: (c: number, p: number, cols: number) => number
}

/** Lit when `v` is within `half` of `target` — the workhorse for strands. */
const near = (v: number, target: number, half: number): number => (Math.abs(v - target) < half ? 1 : 0)

/** A dot travelling the eight-position perimeter of a single cell. */
const ORBIT_PATH: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [1, 0],
  [1, 1],
  [1, 2],
  [1, 3],
  [0, 3],
  [0, 2],
  [0, 1],
]

/**
 * The canonical ten-frame braille spinner, read straight off the characters.
 *
 * Hand-computing these masks is a reliable way to get a spinner that stutters:
 * two frames collide, and the rotation visibly hitches once per cycle. Letting
 * the codepoints be the source of truth removes the arithmetic entirely.
 */
const CLASSIC_MASKS = Array.from('⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏', (c) => c.charCodeAt(0) - 0x2800)

export const ANIMATIONS: WaitingAnimation[] = [
  /* ── Compact: one cell. These fit inline in a sentence. ─────────────── */
  {
    id: 'classic',
    name: 'Braille Classic',
    origin: 'reference',
    blurb: 'The canonical ten-frame spinner. The one everyone recognises as "working".',
    cols: 1,
    period: IMMEDIATE,
    field: (x, y, p) => {
      const mask = CLASSIC_MASKS[Math.floor(frac(p) * CLASSIC_MASKS.length) % CLASSIC_MASKS.length]!
      const bit = y < 3 ? x * 3 + y : 6 + x
      return (mask >> bit) & 1
    },
  },
  {
    id: 'orbit',
    name: 'Orbit',
    origin: 'reference',
    blurb: 'A single dot walking the rim. Calmest thing in the set — good for ambient monitors.',
    cols: 1,
    period: STANDARD,
    field: (x, y, p) => {
      const i = Math.floor(frac(p) * ORBIT_PATH.length) % ORBIT_PATH.length
      const [ox, oy] = ORBIT_PATH[i]!
      return x === ox && y === oy ? 1 : 0
    },
  },
  {
    id: 'breathe',
    name: 'Breathe',
    origin: 'reference',
    blurb: 'Swells from the centre and settles. Reads as alive without reading as busy.',
    // Two cells, not one. A 4×4 dot field has enough distinct radii to show
    // intermediate states; in a single 2×4 cell the same swell quantises to
    // exactly two frames, which is a strobe, not a breath — and a two-state
    // flip is what Mercury already ships as gd-pulse.
    cols: 2,
    period: AMBIENT,
    field: (x, y, p, w) => {
      // Floor of 0.85, not 0: the centre dots stay lit at the bottom of the
      // breath. A waiting indicator that empties completely reads as FINISHED,
      // which is the one thing it must never say.
      const r = 0.85 + 1.7 * (0.5 - 0.5 * Math.cos(frac(p) * Math.PI * 2))
      return Math.hypot((x - (w - 1) / 2) * 1.3, y - 1.5) <= r ? 1 : 0
    },
  },
  {
    id: 'snake',
    name: 'Snake',
    origin: 'reference',
    blurb: 'Orbit with a three-dot tail. The tail is what makes the direction legible.',
    cols: 1,
    period: STANDARD,
    field: (x, y, p) => {
      const head = Math.floor(frac(p) * ORBIT_PATH.length)
      for (let k = 0; k < 3; k++) {
        const [ox, oy] = ORBIT_PATH[(((head - k) % 8) + 8) % 8]!
        if (x === ox && y === oy) return 1
      }
      return 0
    },
  },
  {
    id: 'scan',
    name: 'Scan',
    origin: 'reference',
    blurb: 'A rule sweeping top to bottom. Mercury already uses scan for tool activity.',
    cols: 1,
    period: STANDARD,
    field: (_x, y, p) => (y === Math.floor(frac(p) * 4) % 4 ? 1 : 0),
  },

  /* ── Mid: three cells. The default size for a status row. ───────────── */
  {
    id: 'fill-sweep',
    name: 'Fill Sweep',
    origin: 'reference',
    blurb: 'Floods left to right, then drains the same way. The most "progress"-flavoured one.',
    cols: 3,
    period: STANDARD,
    field: (x, _y, p, w) => {
      const t = frac(p)
      // Both ends clamp one column short of empty, so the strip always has
      // something lit — including at exactly phase 0, where an unclamped fill
      // is zero columns wide.
      return t < 0.5 ? (x < Math.max(1, t * 2 * w) ? 1 : 0) : x >= (t - 0.5) * 2 * (w - 1) ? 1 : 0
    },
  },
  {
    id: 'pulse',
    name: 'Pulse',
    origin: 'reference',
    blurb: 'Opens from the middle outward. Symmetric, so it never implies a direction.',
    cols: 3,
    period: STANDARD,
    field: (x, _y, p, w) => {
      const mid = (w - 1) / 2
      // Bottoms out at 0.5 rather than 0 — mid falls between two dot columns,
      // so a radius of 0 would light nothing and the indicator would blink out.
      const r = 0.5 + (0.5 - 0.5 * Math.cos(frac(p) * Math.PI * 2)) * (mid + 0.5)
      return Math.abs(x - mid) <= r ? 1 : 0
    },
  },
  {
    id: 'columns',
    name: 'Columns',
    origin: 'reference',
    blurb: 'A tiny equaliser. Staggered bars growing from the baseline.',
    cols: 3,
    period: STANDARD,
    field: (x, y, p) => {
      const h = 1 + Math.round(3 * (0.5 - 0.5 * Math.cos(frac(p - x * 0.08) * Math.PI * 2)))
      return y >= 4 - h ? 1 : 0
    },
  },
  {
    id: 'cascade',
    name: 'Cascade',
    origin: 'reference',
    blurb: 'Per-column waterfall on a rolling offset, so the falls read as a diagonal.',
    cols: 3,
    period: STANDARD,
    field: (x, y, p) => {
      const head = frac(p + x * 0.12) * 4
      const d = head - y
      return d >= 0 && d < 1.6 ? 1 : 0
    },
  },
  {
    id: 'diagonal-swipe',
    name: 'Diagonal Swipe',
    origin: 'reference',
    blurb: 'A band raking across on the x+y diagonal. Crisp, mechanical, very legible.',
    cols: 3,
    period: STANDARD,
    field: (x, y, p, w, h) => {
      const pos = frac(p) * (w + h)
      const d = x + y - pos
      return d > -2.5 && d <= 0 ? 1 : 0
    },
  },

  /* ── Wide: four to six cells. Standalone waiting states. ────────────── */
  {
    id: 'checkerboard',
    name: 'Checkerboard',
    origin: 'reference',
    blurb: 'The parity repaints behind a wipe that crosses twice a cycle. High contrast, still directional.',
    cols: 4,
    period: STANDARD,
    field: (x, y, p, w) => {
      // A bare parity flip has exactly two frames — a strobe, and functionally
      // identical to Mercury's gd-pulse fallback. Repainting BEHIND a moving
      // wipe keeps the checkerboard identity but gives the eye a direction to
      // follow, which is what separates "working" from "blinking".
      const parity = frac(p) < 0.5 ? 0 : 1
      const wiped = x < frac(p * 2) * w
      return (x + y) % 2 === (wiped ? 1 - parity : parity) ? 1 : 0
    },
  },
  {
    id: 'rain',
    name: 'Rain',
    origin: 'reference',
    blurb: 'Drops on per-column speeds. Speeds are whole cycles, so the loop has no seam.',
    cols: 4,
    period: AMBIENT,
    field: (x, y, p) => {
      // Integer cycles-per-period: a fractional speed would make every column
      // jump at the phase wrap, which is exactly the seam this set avoids.
      const speed = 1 + Math.floor(hash01(x, 1) * 3)
      const head = frac(p * speed + hash01(x, 2)) * 6 - 1
      const d = head - y
      return d >= 0 && d < 1.2 ? 1 : 0
    },
  },
  {
    id: 'sparkle',
    name: 'Sparkle',
    origin: 'reference',
    blurb: 'Quantised twinkle — twelve steps a cycle, never a 60fps shimmer.',
    cols: 4,
    period: AMBIENT,
    field: (x, y, p) => (hash01(x * 13 + y, Math.floor(frac(p) * 12)) > 0.78 ? 1 : 0),
  },
  {
    id: 'wave-rows',
    name: 'Wave Rows',
    origin: 'reference',
    blurb: 'A travelling sine. The most organic silhouette here.',
    cols: 5,
    period: AMBIENT,
    field: (x, y, p) => near(y, 1.5 + 1.5 * Math.sin(frac(p) * Math.PI * 2 - x * 0.6), 0.9),
  },
  {
    id: 'helix',
    name: 'Helix',
    origin: 'reference',
    blurb: 'Two counter-phase strands crossing. Mercury reserves helix for reasoning loops.',
    cols: 5,
    period: STANDARD,
    field: (x, y, p) => {
      const t = frac(p)
      const a = t * Math.PI * 2 - x * 0.55
      // Antiphase strands repeat every HALF period: at p+0.5 each strand lands
      // exactly where the other one was, so the loop silently runs at double
      // speed. No function of a strand's OWN phase can break that — swapping
      // the strands swaps the thicknesses too. The amplitude has to breathe on
      // absolute phase instead, which it does once per full period.
      const amp = 1.15 + 0.35 * Math.sin(t * Math.PI * 2)
      // Depth: whichever strand is nearer is drawn thicker. This is what makes
      // a helix read as 3D rather than as a flat braid.
      const front = Math.cos(a) > 0
      const s = amp * Math.sin(a)
      return near(y, 1.5 + s, front ? 0.95 : 0.55) || near(y, 1.5 - s, front ? 0.55 : 0.95) ? 1 : 0
    },
  },

  /* ── Original: not in the reference. ─────────────────────────────────── */
  {
    id: 'comet',
    name: 'Comet',
    origin: 'original',
    blurb: 'A bright head with a fading tail — the only one that varies opacity per character.',
    cols: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      // Distance BEHIND the head, measured modulo the strip width, so the tail
      // is still leaving on the right as the head re-enters on the left. A
      // linear sweep instead leaves the strip empty between passes.
      const d = frac((frac(p) * w - x) / w) * w
      if (d > 5) return 0
      // The body thins as it trails: full column at the head, a core stripe by
      // the time it is five dots back.
      return near(y, 1.5, 0.6 + clamp01(1 - d / 5) * 1.6)
    },
    cellAlpha: (c, p, cols) => {
      const w = cols * 2
      const d = frac((frac(p) * w - c * 2) / w) * w
      return 0.14 + 0.86 * clamp01(1 - d / 7)
    },
  },
  {
    id: 'composing',
    name: 'Composing',
    origin: 'original',
    blurb: 'Text accumulating behind a blinking cursor. Says "writing a reply", not "busy".',
    cols: 5,
    period: STANDARD * 2,
    field: (x, y, p, w) => {
      // Starts at 1.5, and column 0 is always "written": otherwise the first
      // instants of the loop are a blinking cursor over nothing, and 40% of
      // that blink is an empty strip.
      const written = 1.5 + frac(p) * (w + 2)
      if (x < written - 1) return y >= 1 && y <= 2 && (x === 0 || hash01(x, 7) > 0.25) ? 1 : 0
      if (x < written) return frac(p * 8) < 0.6 ? 1 : 0
      return 0
    },
  },
  {
    id: 'handoff',
    name: 'Handoff',
    origin: 'original',
    blurb: 'Mass migrating left to right and back. Reads as work passing from you to the agent.',
    cols: 4,
    period: AMBIENT,
    field: (x, _y, p, w) => {
      const t = 0.5 - 0.5 * Math.cos(frac(p) * Math.PI * 2)
      return x < (1 - t) * (w / 2) || x >= w - t * (w / 2) ? 1 : 0
    },
  },
  {
    id: 'ripple',
    name: 'Ripple',
    origin: 'original',
    blurb: 'Sonar rings off centre, dimming outward. The quietest of the wide ones.',
    cols: 5,
    period: AMBIENT,
    field: (x, y, p, w) => {
      const d = Math.hypot((x - (w - 1) / 2) * 0.55, y - 1.5)
      // A persistent emitter at the centre. Rings alone leave the strip empty
      // between wavefronts, and it reads as sonar anyway: something is pinging.
      if (d < 0.75) return 1
      return Math.sin(d * 1.6 - frac(p) * Math.PI * 2) > 0.6 ? 1 : 0
    },
    cellAlpha: (c, _p, cols) => 1 - 0.5 * clamp01(Math.abs(c - (cols - 1) / 2) / ((cols - 1) / 2)),
  },
  {
    id: 'heartbeat',
    name: 'Heartbeat',
    origin: 'original',
    blurb: 'An ECG trace scrolling past. Distinct silhouette — nothing else here has a spike.',
    cols: 6,
    period: AMBIENT,
    field: (x, y, p, w) => {
      // Scrolls by exactly one full width per period, so the trace is seamless.
      const u = x + frac(p) * w
      const r0 = ecgRow(u, w)
      const r1 = ecgRow(u + 1, w)
      return y >= Math.min(r0, r1) && y <= Math.max(r0, r1) ? 1 : 0
    },
  },
  {
    id: 'settle',
    name: 'Settle',
    origin: 'original',
    blurb: 'Columns stack up, hold, then drain. The only one that shows accumulation.',
    cols: 4,
    period: STANDARD * 2,
    field: (x, y, p) => {
      const t = frac(p)
      const fill = t < 0.8 ? (t - hash01(x, 3) * 0.35) / 0.45 : 1 - (t - 0.8) / 0.2
      // Drains to one row, not to nothing — the baseline is what keeps it
      // reading as "still working" through the reset.
      return y >= 4 - Math.max(1, Math.round(clamp01(fill) * 4)) ? 1 : 0
    },
  },
]

/**
 * The ECG waveform as a row index (0 = top). Drawn as a lookup rather than a
 * formula because the shape is the point: flat, Q dip, tall R spike, S dip,
 * flat, rounded T. A sine would just be Wave Rows again.
 */
function ecgRow(u: number, w: number): number {
  const m = ((u % w) + w) % w
  const s = (m / w) * 12
  if (s < 4) return 2
  if (s < 4.5) return 3
  if (s < 5.2) return 0
  if (s < 5.8) return 3
  if (s < 7) return 2
  if (s < 7.8) return 1
  return 2
}

export const REFERENCE_COUNT = ANIMATIONS.filter((a) => a.origin === 'reference').length
export const ORIGINAL_COUNT = ANIMATIONS.filter((a) => a.origin === 'original').length

export const animationById = (id: string): WaitingAnimation | undefined => ANIMATIONS.find((a) => a.id === id)

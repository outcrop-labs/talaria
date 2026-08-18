import { angleGap, clamp01, frac, hash01, type DotField } from './field'
import { AMBIENT, STANDARD } from './animations'

/**
 * Nine waiting states on a 5×5 grid with a brightness per dot.
 *
 * These use the SAME DotField signature as the braille set — the difference is
 * entirely in what the renderer does with the number. Braille thresholds it
 * (a dot is on or off, because a character is one glyph in one colour); the
 * grid keeps it as an opacity. That is why these can hold a gradient across
 * the display and the braille ones cannot.
 *
 * Every dot position is always occupied. Nothing here moves a dot; the dots
 * stay on a fixed lattice and only their brightness changes, which is what
 * makes the family read as one display rather than as scattered particles.
 */

export interface GridAnimation {
  id: string
  name: string
  blurb: string
  /** Grid is n×n. Five matches the reference and is the family's signature. */
  n: number
  period: number
  field: DotField
}

/** Half-plane test rotated k quarter-turns about the centre — the triangle. */
function halfPlane(x: number, y: number, w: number, k: number): number {
  const c = (w - 1) / 2
  let dx = x - c
  let dy = y - c
  for (let i = 0; i < k; i++) {
    const t = dx
    dx = -dy
    dy = t
  }
  return dx + dy <= 0.01 ? 1 : 0
}

/** Position along the border, 0 to 4(n-1), walking clockwise from top-left. */
function perimeterAt(x: number, y: number, n: number): number {
  const last = n - 1
  if (y === 0) return x
  if (x === last) return last + y
  if (y === last) return 2 * last + (last - x)
  return 3 * last + (last - y)
}

export const GRID_ANIMATIONS: GridAnimation[] = [
  {
    id: 'sweep-hand',
    name: 'Sweep Hand',
    blurb: 'A single hand rotating off a lit hub. Reads as a clock, so it reads as elapsed time.',
    n: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      const c = (w - 1) / 2
      const dx = x - c
      const dy = y - c
      const rad = Math.hypot(dx, dy)
      // The hub is always lit — it is also what stops a rotating LINE from
      // repeating every half turn, since a line at θ and θ+π are the same line.
      if (rad < 0.6) return 1
      const d = angleGap(Math.atan2(dy, dx), frac(p) * Math.PI * 2)
      return clamp01(1 - d / 0.55) * clamp01(1.15 - (rad - 0.6) / 3)
    },
  },
  {
    id: 'chevron-spin',
    name: 'Chevron Spin',
    blurb: 'Two arms at a fixed angle, turning. The vertex tells you which way it is pointing.',
    n: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      const c = (w - 1) / 2
      const dx = x - c
      const dy = y - c
      const rad = Math.hypot(dx, dy)
      if (rad < 0.5) return 0.16
      const ang = Math.atan2(dy, dx)
      const base = frac(p) * Math.PI * 2
      const d = Math.min(angleGap(ang, base + 0.75), angleGap(ang, base - 0.75))
      return clamp01(1 - d / 0.5) * clamp01(rad / 1.4)
    },
  },
  {
    id: 'triangle-turn',
    name: 'Triangle Turn',
    blurb: 'A filled half-plane crossfading through four quarter-turns. The heaviest silhouette here.',
    n: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      // Crossfaded rather than snapped: four hard cuts a cycle reads as a
      // stutter, and the whole point of this renderer is that it can blend.
      const t = frac(p) * 4
      const k = Math.floor(t)
      const f = t - k
      return halfPlane(x, y, w, k % 4) * (1 - f) + halfPlane(x, y, w, (k + 1) % 4) * f
    },
  },
  {
    id: 'perimeter-chase',
    name: 'Perimeter Chase',
    blurb: 'A comet running the border over a dim ring. The most literal "still going" of the set.',
    n: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      const last = w - 1
      if (x !== 0 && y !== 0 && x !== last && y !== last) return 0
      const d = frac(perimeterAt(x, y, w) / (4 * last) - frac(p))
      // The dim ring stays visible behind the comet, so the shape never
      // disappears between passes.
      return 0.12 + 0.88 * clamp01(1 - d / 0.3)
    },
  },
  {
    id: 'arc-gap',
    name: 'Arc Gap',
    blurb: 'A ring with a rotating bite taken out of it. Inverse of the chase — motion by absence.',
    n: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      const c = (w - 1) / 2
      const dx = x - c
      const dy = y - c
      const onRing = clamp01(1 - Math.abs(Math.hypot(dx, dy) - 1.8) / 0.95)
      if (onRing <= 0) return 0
      const gap = angleGap(Math.atan2(dy, dx), frac(p) * Math.PI * 2)
      return onRing * (gap < 0.85 ? 0.1 : 1)
    },
  },
  {
    id: 'shape-morph',
    name: 'Shape Morph',
    blurb: 'A superellipse breathing diamond → circle → square. One exponent does all three.',
    n: 5,
    period: AMBIENT,
    field: (x, y, p, w) => {
      const c = (w - 1) / 2
      const dx = Math.abs(x - c) / c
      const dy = Math.abs(y - c) / c
      // |x|^k + |y|^k = 1. k=1 is a diamond, k=2 a circle, k→∞ a square, so the
      // entire morph is one number moving — no shape interpolation needed.
      const k = 1 + 5 * (0.5 - 0.5 * Math.cos(frac(p) * Math.PI * 2))
      const v = Math.pow(dx, k) + Math.pow(dy, k)
      return clamp01(1 - Math.abs(v - 1) / 0.9)
    },
  },
  {
    id: 'dissolve',
    name: 'Dissolve',
    blurb: 'Per-dot noise crossfading between twenty stills. Texture rather than shape.',
    n: 5,
    period: AMBIENT,
    field: (x, y, p) => {
      const STEPS = 20
      const t = frac(p) * STEPS
      const k = Math.floor(t)
      const f = t - k
      const seed = x * 11 + y
      // Crossfading between two noise stills instead of snapping: raw per-frame
      // noise at 60fps is a shimmer that reads as a rendering fault.
      const a = hash01(seed, k % STEPS)
      const b = hash01(seed, (k + 1) % STEPS)
      return 0.1 + 0.9 * (a * (1 - f) + b * f)
    },
  },
  {
    id: 'gradient-rake',
    name: 'Gradient Rake',
    blurb: 'A soft diagonal ramp wrapping through the grid. The smoothest thing in either family.',
    n: 5,
    period: STANDARD,
    field: (x, y, p, w) => {
      const d = frac((x + y) / (2 * (w - 1)) - frac(p))
      return 0.1 + 0.9 * Math.pow(1 - d, 2)
    },
  },
  {
    id: 'pulse-rings',
    name: 'Pulse Rings',
    blurb: 'Concentric wavefronts off the centre. Ambient — it never demands attention.',
    n: 5,
    period: AMBIENT,
    field: (x, y, p, w) => {
      const c = (w - 1) / 2
      const rad = Math.hypot(x - c, y - c)
      const v = Math.sin(rad * 1.7 - frac(p) * Math.PI * 2)
      return 0.1 + 0.9 * Math.pow(clamp01((v + 1) / 2), 2.5)
    },
  },
]

export const gridAnimationById = (id: string): GridAnimation | undefined =>
  GRID_ANIMATIONS.find((a) => a.id === id)

/**
 * THE DITHER VOCABULARY: what a field is made of, and the primitives that
 * rasterise one. No renderer lives here any more.
 *
 * Ordered (Bayer) dithering over a declarative density field is how Mercury
 * gets a gradient without ever painting one. The house is matte by rule — no
 * glows, no blurs — so a halo around a control, or the sense that a surface
 * has material, is carried by the DENSITY of grid-aligned dots rather than by
 * light. The gradient exists statistically and nowhere else.
 *
 * A caller describes a field as a list of SOURCES (edges, haloes around rects,
 * ramps, waves) and hands them to `useField`. The drawing is done on the GPU by
 * `lib/field-gl.ts`, one pass per surface — see `components/ui/FieldSurface`.
 *
 * WHAT USED TO BE HERE. A canvas-2D `DitherEngine` rasterised these same
 * sources on the CPU, one context per control, tweening between states. It was
 * replaced wholesale by the shader: a field is a continuous function of
 * position, state and time, and a per-control raster could neither afford to
 * re-evaluate it nor agree with the CSS approximation running beside it. What
 * survives is this file — the source types, the Bayer matrix, and the lattice
 * and colour helpers — because those describe the field itself rather than any
 * one way of drawing it, and both the shader and the skeleton static share
 * them so their grids stay in phase.
 */

export type DitherTone = 'neutral' | 'accent' | 'success' | 'danger' | 'surface'

interface SourceBase {
  /** Stable identity — tweens match sources across setSources() calls by id. */
  id: string
  /** Peak density contribution, 0..1. */
  strength: number
  tone?: DitherTone
  /**
   * Scale on this source's ALPHA, 0..1. Default 1.
   *
   * Not a second `strength`, and the difference is the whole point. `strength`
   * governs DENSITY — how many cells survive the dither threshold — so lowering
   * it yields FEWER dots at full opacity, which reads as sparse and dotty.
   * `gain` lowers the opacity of the dots that do survive, so the population
   * stays intact and the whole field simply sits further back.
   *
   * Ambient texture needs the second one. A rail's field and a button's halo
   * are the same treatment at very different weights: the rail should read as
   * the surface HAVING MATERIAL, which is a full population at a ghost of the
   * halo's alpha — scattered dots on a panel is a different thing entirely and
   * looks like a rendering fault. Under one shared surface, per-source gain is
   * what keeps both possible, since the alpha ramp itself is per-surface.
   */
  gain?: number
}

/** Density 1 at a container edge, falling off over `depth` px. */
export interface EdgeSource extends SourceBase {
  kind: 'edge'
  side: 'top' | 'bottom' | 'left' | 'right'
  depth: number
}

/** A halo around a rect: peak density at the boundary, falling off over
 *  `spread` px outside. `inner` sets the level deep inside the rect (default
 *  1 — solid). Low `inner` keeps a transparent row's text calm while the rim
 *  still reads: a thin bright band hugs the boundary, then settles. */
export interface RectSource extends SourceBase {
  kind: 'rect'
  x: number
  y: number
  w: number
  h: number
  spread: number
  inner?: number
  /** Depth in px of the boundary-hugging band INSIDE the rect (default 10).
   *  0 removes it — the interior contributes exactly `inner`, so a control
   *  that draws its own border can keep its inside clean for the text. */
  rim?: number
  /** Corner radius in px (default 0 — sharp). Match the control's own
   *  border-radius so the halo's density rings wrap the curve; a sharp field
   *  around a rounded control reads as a square stamped over it. */
  radius?: number
  /** Exponent shaping the outside decay (default 2). Higher starts the
   *  opacity blend-out right at the edge instead of holding near-full for the
   *  first dot rows — a wide, short control (a nav row) needs ~3 to read as
   *  concentric the way a small button does at 2: same treatment, corrected
   *  for geometry. */
  falloff?: number
}

/** Radial falloff around a point. */
export interface HaloSource extends SourceBase {
  kind: 'halo'
  x: number
  y: number
  radius: number
}

/** Linear ramp along one axis: `fromLevel` at/before `from`, `toLevel`
 *  at/after `to`. A progress fill with a dissolving leading edge is one ramp. */
export interface RampSource extends SourceBase {
  kind: 'ramp'
  axis: 'x' | 'y'
  from: number
  to: number
  fromLevel: number
  toLevel: number
}

/** Flat density everywhere — the dissolve veil, or a whisper of grain. */
export interface UniformSource extends SourceBase {
  kind: 'uniform'
}

/** A travelling density crest — the only time-driven source. Negative speed
 *  drifts the other way. Sharpened (cubed sine) so crests read as bands. */
export interface WaveSource extends SourceBase {
  kind: 'wave'
  axis: 'x' | 'y'
  wavelength: number
  /** px per second. */
  speed: number
}

export type DitherSource =
  | EdgeSource
  | RectSource
  | HaloSource
  | RampSource
  | UniformSource
  | WaveSource
export const BAYER = [
   0, 32,  8, 40,  2, 34, 10, 42,
  48, 16, 56, 24, 50, 18, 58, 26,
  12, 44,  4, 36, 14, 46,  6, 38,
  60, 28, 52, 20, 62, 30, 54, 22,
   3, 35, 11, 43,  1, 33,  9, 41,
  51, 19, 59, 27, 49, 17, 57, 25,
  15, 47,  7, 39, 13, 45,  5, 37,
  63, 31, 55, 23, 61, 29, 53, 21,
]

/**
 * Where a canvas sits on the PAGE-WIDE dot lattice.
 *
 * Both fields in this app — the ambient/bloom engine here and the skeleton
 * field in `skeleton-static.ts` — key their grid to the document rather than to
 * their own canvas, so two fields sitting near each other are windows onto one
 * material instead of two patterns up to a pitch out of phase. This is the
 * arithmetic they share.
 *
 * `frac` is how far into a cell the canvas starts (subtract it when placing
 * dots); `cell` is the index of that cell on the page (add it before hashing
 * or reading the Bayer matrix).
 *
 * Captured when geometry is measured, NOT per frame: page coordinates move
 * whenever a scroll container does, and a field re-keyed every scroll frame
 * would crawl.
 */
export function latticeOrigin(pageCoord: number, pitch: number): { frac: number; cell: number } {
  const frac = ((pageCoord % pitch) + pitch) % pitch
  return { frac, cell: Math.round((pageCoord - frac) / pitch) }
}

/** Deterministic 2D+time hash → [0,1) for the shimmer jitter (and for the
 *  skeleton field's per-cell noise — see BAYER above). */
export function hash01(x: number, y: number, t: number): number {
  let h = (x * 374761393 + y * 668265263 + t * 2246822519) | 0
  h = Math.imul(h ^ (h >>> 13), 1274126177)
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296
}

/* ── Token colour resolution ───────────────────────────────────────────── */

export type RGB = [number, number, number]

let scratch: CanvasRenderingContext2D | null = null

/** Parse any CSS colour by letting canvas normalise it — tokens are hex today
 *  but rgba() spellings must not break the engine. */
export function parseColor(css: string): RGB {
  scratch ??= document.createElement('canvas').getContext('2d')!
  scratch.fillStyle = '#000'
  scratch.fillStyle = css.trim()
  const v = String(scratch.fillStyle)
  if (v.startsWith('#')) {
    return [parseInt(v.slice(1, 3), 16), parseInt(v.slice(3, 5), 16), parseInt(v.slice(5, 7), 16)]
  }
  const m = v.match(/rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)/)
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : [128, 128, 128]
}

/* ── Field evaluation ──────────────────────────────────────────────────── */

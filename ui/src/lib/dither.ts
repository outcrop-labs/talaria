/**
 * The dither engine: Bayer-ordered dithering over a declarative density field.
 *
 * Reference: the sidebar video this experiment chases renders soft gradients —
 * a glow along the window chrome, a halo bleeding out of the active nav item —
 * as a field of grid-aligned pixel dots whose DENSITY carries the gradient.
 * That is ordered dithering, and it matters here because Mercury is matte by
 * rule: no glows, no blurs. A dithered field is how an instrument panel gets
 * "glow" without ever painting one — the gradient exists only statistically.
 *
 * The engine is framework-free. Callers describe the field as a list of
 * sources (edges, halos around rects, ramps, waves); the engine rasterises it
 * to a canvas each time it changes, tweening numeric properties between
 * states on Mercury's standard budget so hover and active halos glide rather
 * than pop. Colour is resolved from the live `--theme-*` tokens, never
 * hardcoded, so the same field reads correctly in both modes.
 */

export type DitherTone = 'neutral' | 'accent' | 'success' | 'danger' | 'surface'

interface SourceBase {
  /** Stable identity — tweens match sources across setSources() calls by id. */
  id: string
  /** Peak density contribution, 0..1. */
  strength: number
  tone?: DitherTone
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

export interface DitherEngineOptions {
  /** Grid pitch in CSS px. Default 2 — see `dot`. */
  pitch?: number
  /**
   * Dot size in CSS px (<= pitch). Default 1.
   *
   * THE HOUSE GRAIN IS 2/1, not the 4/2 this started at. Finer dots and more
   * of them read as a material; coarser ones read as a pattern printed on top
   * of the surface. The difference is most obvious where a field is only a few
   * px across — an 8px band at pitch 4 is two rows of dots, which is a dotted
   * outline — but it holds at every size, which is why it is the default
   * rather than something the small cases opt into.
   *
   * Four times the cells per paint is the cost. The engine parks its loop as
   * soon as a field is static, so that is one paint for an idle field, not a
   * per-frame bill.
   */
  dot?: number
  /** Alpha of the sparsest dots. Density scales alpha up toward maxAlpha, so
   *  dense regions read brighter as well as busier — as in the reference. */
  alphaFloor?: number
  maxAlpha?: number
  /** 0 disables. Otherwise a tiny per-cell threshold jitter re-rolled ~6×/s —
   *  the reference field is still, but not dead. Off under reduced motion. */
  shimmer?: number
  /** Cover mode: dots fill the whole cell at alpha 1. For dissolve veils —
   *  density 1 is fully opaque, and the tween IS the ordered dissolve. */
  cover?: boolean
  /** 0..1 — static clump noise multiplied into the field. Pure Bayer renders
   *  gradients as mechanical halftone bands; the reference's texture clusters
   *  and thins organically. 0 (default) for precision fields (meters,
   *  veils); ~0.6 for ambient chrome. Static per cell — it never crawls. */
  organic?: number
  /** Tween budget for source changes, ms. Mercury standard: 150–250. */
  tweenMs?: number
}

/* ── Bayer 8×8 ─────────────────────────────────────────────────────────── */

// The classic index matrix. Threshold for cell (x,y) is (B[i]+0.5)/64, which
// distributes any density 0..1 into an even, clump-free dot pattern — the
// texture in the reference is recognisably this matrix.
//
// EXPORTED, with `hash01` and `parseColor` below, because the skeleton field
// (`lib/skeleton-static.ts`) is a second, deliberately separate engine that
// must nonetheless render the SAME material: one matrix, one hash, one way of
// reading a token colour. A copy there would drift, and the two fields sit
// next to each other on screen.
// prettier-ignore
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

const clamp01 = (v: number): number => (v < 0 ? 0 : v > 1 ? 1 : v)

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

const TONE_VAR: Record<DitherTone, string> = {
  neutral: '--theme-text',
  accent: '--theme-accent',
  success: '--theme-success',
  danger: '--theme-danger',
  surface: '--theme-panel',
}

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

function resolveTones(): Record<DitherTone, RGB> {
  const style = getComputedStyle(document.documentElement)
  const out = {} as Record<DitherTone, RGB>
  for (const tone of Object.keys(TONE_VAR) as DitherTone[]) {
    out[tone] = parseColor(style.getPropertyValue(TONE_VAR[tone]) || '#808080')
  }
  return out
}

/* ── Field evaluation ──────────────────────────────────────────────────── */

/**
 * The density one source contributes at a point, 0..1.
 *
 * EXPORTED FOR TESTS. Everything else in this file needs a canvas, a DPR and a
 * rAF loop; this is the half that is pure arithmetic, and it is also the half
 * that carries the intent — the rounded-rect signed distance, the clean
 * interior a label sits on, the falloff exponent that makes a wide nav row
 * read like a small button. Those degrade silently: a wrong exponent still
 * paints a field, just not the right one.
 */
export function evalSource(s: DitherSource, x: number, y: number, t: number, w: number, h: number): number {
  switch (s.kind) {
    case 'uniform':
      return s.strength
    case 'edge': {
      const d = s.side === 'top' ? y : s.side === 'bottom' ? h - y : s.side === 'left' ? x : w - x
      const f = clamp01(1 - d / s.depth)
      // Shaped falloff: denser right at the edge, a long quiet tail — the
      // linear ramp reads mechanical next to the reference.
      return s.strength * Math.pow(f, 2.1)
    }
    case 'rect': {
      // Signed distance to the ROUNDED rect: negative inside (|sd| = depth),
      // positive outside. At radius 0 this is exactly the sharp-rect field.
      const r = Math.min(s.radius ?? 0, s.w / 2, s.h / 2)
      const qx = Math.max(s.x + r - x, x - (s.x + s.w - r))
      const qy = Math.max(s.y + r - y, y - (s.y + s.h - r))
      const sd = Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r
      if (sd < 0) {
        // Inside: a thin rim at full strength hugging the boundary, settling
        // to `inner` toward the middle so overlaid text stays readable.
        const inner = s.inner ?? 1
        if (inner >= 1) return s.strength
        const rimDepth = s.rim ?? 10
        if (rimDepth <= 0) return s.strength * inner
        const rim = clamp01(1 + sd / rimDepth)
        return s.strength * (inner + (1 - inner) * rim * rim)
      }
      if (sd >= s.spread) return 0
      const f = 1 - sd / s.spread
      return s.strength * Math.pow(f, s.falloff ?? 2)
    }
    case 'halo': {
      const d = Math.hypot(x - s.x, y - s.y)
      if (d >= s.radius) return 0
      const f = 1 - d / s.radius
      return s.strength * f * f
    }
    case 'ramp': {
      const c = s.axis === 'x' ? x : y
      const p = s.to === s.from ? 1 : clamp01((c - s.from) / (s.to - s.from))
      return s.strength * (s.fromLevel + (s.toLevel - s.fromLevel) * p)
    }
    case 'wave': {
      const c = s.axis === 'x' ? x : y
      const crest = 0.5 + 0.5 * Math.sin(((c - s.speed * t) / s.wavelength) * Math.PI * 2)
      return s.strength * crest * crest * crest
    }
  }
}

/* ── Tweening ──────────────────────────────────────────────────────────── */

const easeOutCubic = (p: number): number => 1 - Math.pow(1 - p, 3)

interface Entry {
  from: DitherSource
  to: DitherSource
  t0: number
  /** A removed source tweening its strength out; dropped at tween end. */
  ghost: boolean
}

/** Interpolate every numeric property the two snapshots share; non-numeric
 *  (kind, side, axis, tone) comes from the target. */
function lerpSource(from: DitherSource, to: DitherSource, e: number): DitherSource {
  const out = { ...to }
  const writable = out as unknown as Record<string, number>
  for (const key of Object.keys(to) as Array<keyof DitherSource>) {
    const a = from[key]
    const b = to[key]
    if (typeof a === 'number' && typeof b === 'number') writable[key] = a + (b - a) * e
  }
  return out
}

/* ── Engine ────────────────────────────────────────────────────────────── */

export class DitherEngine {
  private canvas: HTMLCanvasElement
  private ctx: CanvasRenderingContext2D
  private opts: Required<DitherEngineOptions>
  private tones: Record<DitherTone, RGB>
  private entries = new Map<string, Entry>()
  private ghostSeq = 0
  private wCss = 0
  private hCss = 0
  // Position on the page-wide lattice — see `latticeOrigin`.
  private fx = 0
  private fy = 0
  private ox = 0
  private oy = 0
  private raf = 0
  private reduced = false
  private lastShimmerBucket = -1
  private destroyed = false

  constructor(canvas: HTMLCanvasElement, opts: DitherEngineOptions = {}) {
    this.canvas = canvas
    this.ctx = canvas.getContext('2d')!
    this.opts = {
      pitch: opts.pitch ?? 2,
      dot: opts.dot ?? 1,
      alphaFloor: opts.alphaFloor ?? 0.18,
      maxAlpha: opts.maxAlpha ?? 0.55,
      shimmer: opts.shimmer ?? 0,
      cover: opts.cover ?? false,
      organic: opts.organic ?? 0,
      tweenMs: opts.tweenMs ?? 200,
    }
    this.tones = resolveTones()
  }

  setSize(wCss: number, hCss: number, dpr: number): void {
    if (wCss <= 0 || hCss <= 0) return
    this.wCss = wCss
    this.hCss = hCss

    // Re-key to the page lattice. Read from the canvas itself so a bled layer
    // (whose canvas extends past its container) lands correctly without the
    // caller having to subtract the bleed a second time.
    const rect = this.canvas.getBoundingClientRect()
    const gx = latticeOrigin(rect.left + window.scrollX, this.opts.pitch)
    const gy = latticeOrigin(rect.top + window.scrollY, this.opts.pitch)
    this.fx = gx.frac
    this.fy = gy.frac
    this.ox = gx.cell
    this.oy = gy.cell
    this.canvas.width = Math.round(wCss * dpr)
    this.canvas.height = Math.round(hCss * dpr)
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    this.schedule()
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced
    this.schedule()
  }

  /** The one live-settable option: shimmer is presence-dependent (a field may
   *  hold still until the pointer is near), and rebuilding the engine to
   *  change it would kill in-flight tweens. */
  setShimmer(shimmer: number): void {
    if (this.opts.shimmer === shimmer) return
    this.opts.shimmer = shimmer
    this.schedule()
  }

  /** Theme flipped — the tokens the tones resolved from have new values. */
  refreshColors(): void {
    this.tones = resolveTones()
    this.schedule()
  }

  setSources(sources: DitherSource[], o?: { immediate?: boolean }): void {
    const now = performance.now()
    const immediate = o?.immediate || this.reduced
    const seen = new Set<string>()

    for (const next of sources) {
      seen.add(next.id)
      const prev = this.entries.get(next.id)
      if (immediate || !prev || prev.to.kind !== next.kind) {
        // A kind change can't interpolate — let the old shape tween out as a
        // ghost under a synthetic key while the new one fades in.
        if (prev && prev.to.kind !== next.kind && !immediate) {
          this.entries.set(`${next.id}~out${this.ghostSeq++}`, {
            from: this.snapshot(prev, now),
            to: { ...prev.to, strength: 0 },
            t0: now,
            ghost: true,
          })
        }
        this.entries.set(next.id, {
          from: immediate ? next : { ...next, strength: prev ? this.snapshot(prev, now).strength : 0 },
          to: next,
          t0: now,
          ghost: false,
        })
      } else {
        this.entries.set(next.id, { from: this.snapshot(prev, now), to: next, t0: now, ghost: false })
      }
    }

    for (const [id, entry] of this.entries) {
      if (seen.has(id) || entry.ghost) continue
      if (immediate) this.entries.delete(id)
      else this.entries.set(id, { from: this.snapshot(entry, now), to: { ...entry.to, strength: 0 }, t0: now, ghost: true })
    }

    this.schedule()
  }

  destroy(): void {
    this.destroyed = true
    cancelAnimationFrame(this.raf)
  }

  /** Where an in-flight tween currently sits — retargeting starts from here,
   *  not from the tween's original endpoints, so interrupts don't jump. */
  private snapshot(entry: Entry, now: number): DitherSource {
    const e = easeOutCubic(clamp01((now - entry.t0) / this.opts.tweenMs))
    return lerpSource(entry.from, entry.to, e)
  }

  private schedule(): void {
    if (this.destroyed || this.raf) return
    this.raf = requestAnimationFrame((now) => {
      this.raf = 0
      this.frame(now)
    })
  }

  private frame(now: number): void {
    if (this.destroyed) return

    let tweening = false
    for (const [id, entry] of this.entries) {
      if (now - entry.t0 < this.opts.tweenMs) tweening = true
      else if (entry.ghost) this.entries.delete(id)
    }

    const hasWave = !this.reduced && [...this.entries.values()].some((e) => e.to.kind === 'wave')
    const shimmering = !this.reduced && this.opts.shimmer > 0 && this.entries.size > 0

    // Shimmer repaints on its own slow clock (~6/s); don't burn full-rate
    // paints when the jitter bucket hasn't advanced and nothing else moves.
    const bucket = Math.floor(now / 160)
    const shimmerAdvanced = bucket !== this.lastShimmerBucket

    if (tweening || hasWave || (shimmering && shimmerAdvanced)) {
      this.lastShimmerBucket = bucket
      this.paint(now)
    } else if (!shimmering) {
      this.paint(now)
      return // fully static — stop the loop
    }

    if (tweening || hasWave || shimmering) this.schedule()
  }

  private paint(now: number): void {
    const { ctx, wCss, hCss, tones } = this
    const { pitch, dot, alphaFloor, maxAlpha, shimmer, cover, organic } = this.opts
    if (wCss === 0 || hCss === 0) return

    ctx.clearRect(0, 0, wCss, hCss)
    if (this.entries.size === 0) return

    const t = now / 1000
    const active: DitherSource[] = []
    for (const entry of this.entries.values()) {
      const s = this.snapshot(entry, now)
      if (s.strength > 0.002) active.push(s)
    }
    if (active.length === 0) return

    // `+ frac` because the grid starts up to one pitch before this canvas —
    // without it the last column/row on the far edge would be dropped.
    const cols = Math.ceil((wCss + this.fx) / pitch)
    const rows = Math.ceil((hCss + this.fy) / pitch)
    const size = cover ? pitch : dot
    const off = cover ? 0 : (pitch - dot) / 2
    const bucket = Math.floor(now / 160)

    for (let cy = 0; cy < rows; cy++) {
      const y = cy * pitch - this.fy + pitch / 2
      // Page cell indices: the matrix and the noise are read from these, so the
      // pattern is continuous across every field on the page. The FIELD is
      // still sampled in local coordinates, because sources are local.
      const gy = this.oy + cy
      for (let cx = 0; cx < cols; cx++) {
        const x = cx * pitch - this.fx + pitch / 2
        const gx = this.ox + cx

        // Screen-accumulate density; colour is the tone mix weighted by each
        // source's contribution, so an accent halo tints only where it lives.
        let miss = 1
        let r = 0
        let g = 0
        let b = 0
        let wsum = 0
        for (const s of active) {
          const v = evalSource(s, x, y, t, wCss, hCss)
          if (v <= 0) continue
          miss *= 1 - clamp01(v)
          const c = tones[s.tone ?? 'neutral']
          r += c[0] * v
          g += c[1] * v
          b += c[2] * v
          wsum += v
        }
        if (wsum === 0) continue

        let density = 1 - miss
        if (organic > 0) {
          // Two octaves — 4-cell blocks give the clusters, per-cell breaks
          // their edges. Seeds are constants: the clumps never move.
          const clump = 0.6 * hash01(gx >> 2, gy >> 2, 7) + 0.4 * hash01(gx, gy, 13)
          density *= 1 + organic * (clump * 1.8 - 0.9)
        }
        if (shimmer > 0 && !this.reduced && density > 0.03 && density < 0.97) {
          density += (hash01(gx, gy, bucket) - 0.5) * shimmer
        }
        if (density <= (BAYER[(gy & 7) * 8 + (gx & 7)]! + 0.5) / 64) continue

        const alpha = cover ? 1 : alphaFloor + (maxAlpha - alphaFloor) * clamp01(density)
        ctx.fillStyle = `rgba(${Math.round(r / wsum)},${Math.round(g / wsum)},${Math.round(b / wsum)},${alpha})`
        ctx.fillRect(cx * pitch - this.fx + off, cy * pitch - this.fy + off, size, size)
      }
    }
  }
}

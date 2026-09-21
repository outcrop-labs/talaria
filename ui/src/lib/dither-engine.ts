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

// NAMED `-engine` SO THE DESKTOP LAUNCHER CAN SHARE IT. It imports NOTHING —
// no `@/` alias, no waiting field, no framework — because desktop/ (a separate
// package with its own vite config) aliases this file directly. `clamp01` is
// local for that reason: the waiting field's copy was byte-identical anyway,
// and `hash01` stays per-engine on purpose (see the NOTE in ui/src/lib/waiting/field.ts).
const clamp01 = (x: number): number => Math.min(1, Math.max(0, x))

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

/** A rect in the engine canvas's own CSS-px space, for `setMask`. */
export interface MaskRect {
  x: number
  y: number
  w: number
  h: number
}

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
   * Four times the cells is four times the work when a field is BUILT — mount,
   * resize, a source change — and not a per-frame bill: a static field parks
   * its loop, and a live one repaints only the cells whose pixel changes (see
   * `paint`).
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
/**
 * Alpha of an unlit cell, relative to a lit one.
 *
 * From dither-kit (MIT): drawing the gaps faintly instead of leaving them
 * empty is what keeps a dithered field reading as one material rather than as
 * dots scattered on a surface — and it is what makes a state change a change
 * in degree rather than an appearance.
 */
export const OFF_TIER = 0.4

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
  private lastWaveBucket = -1
  private destroyed = false
  private mask: MaskRect[] | null = null
  // ── what a frame has to do, and what it can know ──
  //
  // The field is a function of the sources' GEOMETRY, and geometry only moves
  // when a tween is in flight or a wave is travelling. Everything else a frame
  // touches — the shimmer's re-rolled threshold, the lit/unlit step it may
  // cause — is a decision per cell, not a re-evaluation of the field. These
  // four hold that split: `density`/`ink` are the field itself, `painted` is
  // what is on the canvas cell by cell, and `styles` keeps the colour strings
  // a cell can ask for.
  private density: Float32Array | null = null
  private ink: Uint32Array | null = null
  private painted: Uint32Array | null = null
  private styles = new Map<number, string>()
  private cacheDirty = true
  private drew = false
  // Never paint a field nobody can see — see the constructor.
  private inView = true
  private docVisible = true
  private io: IntersectionObserver | null = null
  private onDocVisibility: (() => void) | null = null

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

    // NEVER PAINT A FIELD NOBODY CAN SEE. The desktop shell is the reason this
    // lives in the engine rather than in a wrapper: the launcher webview stays
    // ALIVE while an instance holds the window (hiding it is what keeps its
    // session), so a full-window field kept repainting there at the cost of the
    // whole window — and the same is true of any field scrolled out of its pane
    // in the product. Both signals are watched here, so no call site can forget
    // one: an invisible field stops its loop, and becoming visible again
    // repaints from scratch (the canvas cannot be trusted across the gap — the
    // window may have been resized or repainted by the compositor while away).
    if (typeof IntersectionObserver === 'function') {
      this.io = new IntersectionObserver((entries) => {
        const last = entries[entries.length - 1]
        if (!last || last.isIntersecting === this.inView) return
        this.inView = last.isIntersecting
        if (this.inView) {
          this.invalidate()
          this.schedule()
        }
      })
      this.io.observe(canvas)
    }
    if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
      this.docVisible = !document.hidden
      this.onDocVisibility = () => {
        if (document.hidden === !this.docVisible) return
        this.docVisible = !document.hidden
        if (this.docVisible) {
          this.invalidate()
          this.schedule()
        }
      }
      document.addEventListener('visibilitychange', this.onDocVisibility)
    }
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
    this.invalidate()
    this.schedule()
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced
    this.invalidate()
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
    this.styles.clear()
    this.invalidate()
    this.schedule()
  }

  /**
   * Clip subsequent paints to these rects (the canvas's own CSS-px space).
   *
   * This is how a field FOLLOWS something smaller than the canvas — the
   * dither that condenses out of the scrambling tail of streaming text: the
   * tail moves and the field moves with it, and every settled character
   * outside the mask stays clean text. `null` paints the whole canvas; an
   * EMPTY list paints nothing.
   */
  setMask(mask: MaskRect[] | null): void {
    this.mask = mask
    // Cells outside a new mask have to be erased, and the incremental path
    // cannot tell which ones were drawn under the old one — so the field is
    // rebuilt from scratch, clear included.
    this.invalidate()
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

    this.invalidate()
    this.schedule()
  }

  destroy(): void {
    this.destroyed = true
    cancelAnimationFrame(this.raf)
    this.io?.disconnect()
    this.io = null
    if (this.onDocVisibility && typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.onDocVisibility)
      this.onDocVisibility = null
    }
    this.density = null
    this.ink = null
    this.painted = null
    this.styles.clear()
  }

  /** The field has to be rebuilt from a clean canvas: geometry, sources, theme
   *  or motion changed, or the field has been away and its pixels cannot be
   *  trusted. */
  private invalidate(): void {
    this.cacheDirty = true
    this.painted = null
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
    // Invisible: drop the loop. A visibility flip invalidates and reschedules,
    // so the field comes back painted rather than stale.
    if (!this.inView || !this.docVisible) return

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

    // Waves ride a bucketed clock too (~12/s). A wave's speed is single-digit
    // pixels per second — a sub-pixel step per bucket — so a bucketed wave is
    // visually identical to a full-rate one, and paint is the app's single
    // most expensive standing cost: an always-mounted wave field (the brief
    // hero) at full rAF was ~80% of a core, forever, on the machine of
    // anyone who parked on Home (the 2026-09-14 "browser crawls after a
    // while" report measured 47.5s of script per minute on an idle tab).
    // Tweens keep full rate — they are short and they are the transition
    // itself.
    const waveBucket = Math.floor(now / 80)
    const waveAdvanced = waveBucket !== this.lastWaveBucket

    if (tweening || hasWave || shimmering) {
      if (tweening || (hasWave && waveAdvanced) || (shimmering && shimmerAdvanced)) {
        this.lastShimmerBucket = bucket
        this.lastWaveBucket = waveBucket
        // A tween or a travelling wave moves the FIELD under the frame; a
        // shimmer tick only re-rolls thresholds. Only the first has to
        // re-evaluate the sources.
        this.paint(now, tweening || hasWave)
      }
      this.schedule()
    } else {
      this.paint(now, false)
      // fully static — stop the loop
    }
  }

  /**
   * One frame. `live` says the field's own geometry is moving under it — a
   * tween mid-flight, or a travelling wave — so density has to be re-evaluated
   * from the sources. Otherwise the field is a cached PICTURE and the frame
   * only has to decide, cell by cell, which pixels the re-rolled threshold
   * changes: those are the only ones it writes.
   *
   * That split is this engine's whole performance story, and it is worth the
   * paragraph. The house grain (pitch 2 / dot 1) is four times the cells of the
   * 4/2 this started at, and a full-window field is a quarter of a million of
   * them; writing every cell on every tick cost the desktop shell's launcher
   * (a full-window field that stays mounted — and repainting — behind an active
   * instance) ~600ms of main thread per frame, and full-pane empty states ~80%
   * of a core. Nothing about a static field changes between two shimmer ticks
   * except a threshold, so nothing else is recomputed and nothing else is
   * redrawn.
   */
  private paint(now: number, live: boolean): void {
    const { ctx, wCss, hCss, tones } = this
    const { pitch, dot, alphaFloor, maxAlpha, shimmer, cover, organic } = this.opts
    if (wCss === 0 || hCss === 0) return

    // `+ frac` because the grid starts up to one pitch before this canvas —
    // without it the last column/row on the far edge would be dropped.
    const cols = Math.ceil((wCss + this.fx) / pitch)
    const rows = Math.ceil((hCss + this.fy) / pitch)
    const cells = cols * rows
    const size = cover ? pitch : dot
    const off = cover ? 0 : (pitch - dot) / 2
    const bucket = Math.floor(now / 160)
    const t = now / 1000

    // THE CANVAS IS THE STATE. `painted` is what is on it, cell by cell, packed
    // into the same 8 bits per channel the canvas stores — so two cells that
    // pack equal are two cells that PAINT equal, and the frame can write only
    // the ones that differ. A length change means the geometry moved under us:
    // clear, and start the picture over.
    let painted = this.painted
    if (!painted || painted.length !== cells) {
      ctx.clearRect(0, 0, wCss, hCss)
      painted = this.painted = new Uint32Array(cells)
      this.drew = false
    }

    const active: DitherSource[] = []
    for (const entry of this.entries.values()) {
      const s = this.snapshot(entry, now)
      if (s.strength > 0.002) active.push(s)
    }

    // A mask is a promise about WHERE: an empty list means the caller has
    // nothing to field, and a cleared canvas is the correct answer — including
    // when a field that HAD ink loses its last source.
    const mask = this.mask
    if (active.length === 0 || (mask !== null && mask.length === 0)) {
      if (this.drew) {
        ctx.clearRect(0, 0, wCss, hCss)
        painted.fill(0)
        this.drew = false
      }
      return
    }

    ctx.save()
    if (mask && mask.length > 0) {
      ctx.beginPath()
      for (const r of mask) ctx.rect(r.x, r.y, r.w, r.h)
      ctx.clip()
    }

    // THE FIELD IS GEOMETRY, so it is computed once and read per tick: a
    // shimmer tick re-rolls a threshold, not a source. `density`/`ink` hold the
    // picture for every frame that is not live; a live frame recomputes it and
    // leaves the cache dirty, so the frame that settles the field rebuilds it
    // once.
    const useCache =
      !live &&
      !this.cacheDirty &&
      this.density !== null &&
      this.ink !== null &&
      this.density.length === cells &&
      this.ink.length === cells
    // READ the picture, or BUILD it — never both. A frame that builds reads
    // nothing (the buffers it just allocated are empty), which is the one way
    // this pair can be miswired: reading a fresh buffer paints an empty field.
    const densityOf = useCache ? this.density! : null
    const inkOf = useCache ? this.ink! : null
    const building = !live && !useCache
    const writeDensity = building ? new Float32Array(cells) : null
    const writeInk = building ? new Uint32Array(cells) : null
    const shimmering = shimmer > 0 && !this.reduced
    const halfShimmer = shimmer / 2

    for (let cy = 0; cy < rows; cy++) {
      const y = cy * pitch - this.fy + pitch / 2
      const top = cy * pitch - this.fy + off
      // Page cell indices: the matrix and the noise are read from these, so the
      // pattern is continuous across every field on the page. The FIELD is
      // still sampled in local coordinates, because sources are local.
      const gy = this.oy + cy
      for (let cx = 0; cx < cols; cx++) {
        const i = cy * cols + cx
        const x = cx * pitch - this.fx + pitch / 2
        const gx = this.ox + cx

        let density = 0
        let ink = 0
        if (densityOf) {
          density = densityOf[i]!
          ink = inkOf![i]!
        } else {
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
          if (wsum > 0) {
            density = 1 - miss
            if (organic > 0) {
              // Two octaves — 4-cell blocks give the clusters, per-cell breaks
              // their edges. Seeds are constants: the clumps never move, which
              // is why they belong to the cached picture rather than to every
              // tick that reads it.
              const clump = 0.6 * hash01(gx >> 2, gy >> 2, 7) + 0.4 * hash01(gx, gy, 13)
              density *= 1 + organic * (clump * 1.8 - 0.9)
            }
            ink = (Math.round(r / wsum) << 16) | (Math.round(g / wsum) << 8) | Math.round(b / wsum)
          }
          if (writeDensity && writeInk) {
            writeDensity[i] = density
            writeInk[i] = ink
          }
        }

        // Nothing here — a cell that used to hold ink is erased, and the rest
        // are left exactly as they are.
        if (density <= 0.002) {
          if (painted[i] !== 0) {
            ctx.clearRect(cx * pitch - this.fx + off, top, size, size)
            painted[i] = 0
          }
          continue
        }

        // TWO TIERS, NOT DOTS AND HOLES.
        //
        // Adopted from dither-kit (MIT, Boring-Software-Inc/dither-kit), whose
        // engine puts the rule plainly: the scatter modulates between two tiers
        // of the SAME colour rather than leaving holes, so nothing shows the
        // background through. The CSS tile does this now and the canvas fields
        // have to agree, or the two paths read as different materials.
        //
        // It also softens every change of state. A cell that crosses the
        // threshold steps from the faint tier to the full one — a change in
        // DEGREE — where before it appeared out of nothing, which the eye
        // catches as an event.
        //
        // A cell with no field on it at all is still skipped: the tier is a
        // floor under the texture, not a wash over the whole surface.
        const threshold = (BAYER[(gy & 7) * 8 + (gx & 7)]! + 0.5) / 64

        // SHIMMER IS A THRESHOLD JITTER, and it is paid for only where a flip
        // is possible: the jitter moves the threshold by at most half a
        // shimmer, so a cell further than that from its own threshold cannot
        // change and neither the hash nor the redraw is spent on it. The ALPHA
        // deliberately does not ride the jitter — it is a function of density
        // alone. A jitter that moved every cell's alpha by a level would make
        // every pixel on the field differ on every tick, which is the cost this
        // whole frame exists to avoid; the visible sparkle is the lit/unlit
        // step, which is exactly the part that is kept.
        const lit =
          shimmering &&
          density > 0.03 &&
          density < 0.97 &&
          Math.abs(density - threshold) <= halfShimmer
            ? density > threshold - (hash01(gx, gy, bucket) - 0.5) * shimmer
            : density > threshold

        // The unlit tier is scaled by density rather than lifted off
        // `alphaFloor`, so it fades out exactly where the field does instead of
        // leaving a faint rectangle at the field's edge.
        const alpha = cover
          ? 1
          : lit
            ? alphaFloor + (maxAlpha - alphaFloor) * clamp01(density)
            : maxAlpha * clamp01(density) * OFF_TIER
        const packed = ((ink << 8) | (cover ? 255 : Math.round(alpha * 255))) >>> 0
        if (packed === painted[i]) continue
        ctx.fillStyle = this.styleOf(packed)
        ctx.fillRect(cx * pitch - this.fx + off, top, size, size)
        painted[i] = packed
        this.drew = true
      }
    }
    ctx.restore()

    this.density = writeDensity ?? densityOf ?? this.density
    this.ink = writeInk ?? inkOf ?? this.ink
    // A LIVE frame's picture is not cacheable, so the frame that settles the
    // field rebuilds it once. A frame that just BUILT it leaves it valid, and a
    // frame that merely READ it must not throw it away — that mistake made the
    // cache useless: every other tick paid a full re-evaluation of the field.
    this.cacheDirty = live
  }

  /** The colour string for a packed pixel, built once per distinct pixel. The
   *  canvas quantises alpha to 8 bits anyway, so the key is exact: a field has
   *  a handful of distinct pixels, not one per cell, and every `fillStyle`
   *  assignment otherwise re-allocates a string for the CSS colour parser. */
  private styleOf(packed: number): string {
    let style = this.styles.get(packed)
    if (style === undefined) {
      style = `rgba(${(packed >>> 24) & 255},${(packed >>> 16) & 255},${(packed >>> 8) & 255},${(packed & 255) / 255})`
      this.styles.set(packed, style)
    }
    return style
  }
}

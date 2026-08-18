/**
 * SIGNAL STATIC — the material every loading skeleton is made of.
 *
 * A skeleton used to be a surface-toned block breathing on the ambient budget.
 * It now renders as a dithered dot field: the same Bayer 8×8 grid, the same
 * token colour and the same pitch as `lib/dither.ts`, so a skeleton and the
 * ambient chrome around it read as one material rather than two effects.
 *
 * Every field on the page now runs at the same 2px pitch — skeletons, the
 * controls' bands, the ambient chrome — so they share one lattice literally
 * rather than approximately: same boundaries, same dot centres, anchored to
 * page coordinates through `latticeOrigin`.
 *
 * The motif is AN INSTRUMENT THAT HAS NOT ACQUIRED ITS SIGNAL YET. Static has
 * no direction, no travel and no stagger — per-cell noise re-rolled at 8Hz
 * around a steady mean. That is deliberate: a skeleton is ambient waiting, not
 * progress, and anything that sweeps or fills implies a completion the fetch
 * cannot promise. It is alive the way an idle instrument is alive.
 *
 * This is NOT `DitherEngine`, and the split is on purpose. That engine composes
 * additive ambient sources (edges, halos, waves) over a whole surface and
 * tweens between states; a skeleton is one hard shape mask times exactly one
 * animation, painted for many small elements at once. What the two DO share —
 * matrix, hash, colour parsing — is imported rather than copied.
 *
 * ONE TICKER, MANY ELEMENTS. Every `<Skeleton>` on the page registers here and
 * is painted from the app's single rAF loop (`lib/waiting`'s clock), bucketed
 * down to 8Hz. A pane full of skeleton rows is one subscription and one set of
 * observers, not forty. Offscreen fields don't paint at all, and a background
 * tab stops the clock for everyone because rAF stops.
 *
 * ONE LATTICE. Cell coordinates are keyed to the page, not to the element, so
 * the dot grid and the noise are continuous across neighbouring skeletons —
 * five rows of a loading list are five windows onto one field, not five
 * independent patches. `DitherEngine` keys to the same lattice through the
 * same `latticeOrigin`, so a skeleton and a button's bloom sitting near each
 * other are one material rather than two patterns out of phase.
 *
 * Under reduced motion the CLOCK stops, not the material: the field freezes on
 * one roll of the noise rather than flattening to a mean, so it is recognisably
 * the same texture standing still. Still loading, still legible as loading.
 */

import { BAYER, hash01, latticeOrigin, parseColor, type RGB } from './dither'
import { onReducedMotion, subscribeToClock } from './motion'
import { onThemeChange } from './theme'

/* ── Constants ─────────────────────────────────────────────────────────── */

/**
 * Grid pitch / dot size in CSS px, matching `dither.ts`'s default.
 *
 * Finer than the 4/2 this started at, and the reason is measurement rather
 * than taste: a 200x10 bar and a 200x64 block came out at the SAME mean alpha,
 * so a thin bar was never a density problem that could be softened — at 10px,
 * two or three rows of 2px dots read as a dotted outline at any density, and
 * pitch was the only lever. Halving it puts four or five rows in the same bar.
 *
 * Coverage is unchanged by the swap: ~48% of cells lit, each dot covering
 * (DOT/PITCH)^2 of its cell, is 12% either way. The field is the same weight,
 * finer grained.
 *
 * Not a per-call-site choice — one pitch is what makes the page one lattice.
 */
export const PITCH = 2
const DOT = 1
/** Density scales alpha as well as dot count, so dense regions read brighter. */
const ALPHA_FLOOR = 0.2
const MAX_ALPHA = 0.62
/** The level the noise swings around, and the width of that swing. */
const MEAN = 0.48
const SWING = 0.55
/**
 * Below this many masked cells a field is too small to hold a pattern.
 *
 * Counted in CELLS, not pixels, because it is the pattern that needs room: the
 * ramp asks "can half of these read as texture", and that is a question about
 * how many there are. Halving the pitch quadrupled the cells in any given
 * element, so this was rescaled with it to keep the floor at the same PHYSICAL
 * size — about a 20px box, which is where the measured curve stops changing.
 */
const PATTERN_CELLS = 96
/** 8Hz. Frames between buckets are skipped so the field flickers, not boils. */
const BUCKET_MS = 125

/**
 * Which 8Hz tick a clock reading falls in.
 *
 * EXPORTED FOR TESTS. The page clock runs at frame rate and this is the whole
 * of the throttle: equal buckets mean "nothing to repaint". Get it wrong in
 * either direction and the field still animates — it just boils at 60Hz or
 * crawls — which is the kind of thing no one notices in review.
 */
export function tickBucket(elapsedMs: number): number {
  return Math.floor(elapsedMs / BUCKET_MS)
}

const clampUnit = (v: number): number => (v < 0 ? 0 : v > 1 ? 1 : v)

/* ── The field, as arithmetic ──────────────────────────────────────────── */

/**
 * Density at one cell, 0..1 — the whole animation.
 *
 * EXPORTED FOR TESTS, like `evalSource`: everything else in this file needs a
 * canvas, a DPR and a rAF loop, and this is the half that carries the intent.
 * `bucket` is the 8Hz clock tick; holding it constant freezes the field, which
 * is exactly what reduced motion does.
 */
export function staticDensity(cx: number, cy: number, bucket: number): number {
  return MEAN + (hash01(cx, cy, bucket) - 0.5) * SWING
}

/**
 * How far a field has to give up on texture, 0..1, from how many cells its
 * mask covers.
 *
 * Signal static is a STATISTICAL material: at the 0.48 mean, roughly half the
 * cells light. That reads as texture across a text bar and as nothing at all
 * across a 6px status dot, where half of two cells is regularly zero — and at
 * 8Hz the dot does not merely look thin, it blinks out of existence. The old
 * solid block never could.
 *
 * So a field that has no room for a pattern stops trying to carry one and goes
 * solid instead, easing between the two rather than switching at a threshold —
 * a 16px avatar and a 20px one sit in the same list and must not look like two
 * different materials.
 *
 * EXPORTED FOR TESTS.
 */
export function solidity(cells: number): number {
  return clampUnit((PATTERN_CELLS - cells) / PATTERN_CELLS)
}

/**
 * Is this cell inside the skeleton's rounded box? The signed-distance formula
 * from `dither.ts`, with the radius clamped to the half-size so any oversized
 * value (Tailwind's `rounded-full` is `calc(infinity * 1px)`) reads as a
 * capsule or a circle.
 */
export function insideRounded(x: number, y: number, w: number, h: number, r: number): boolean {
  const rr = Math.min(r, w / 2, h / 2)
  const qx = Math.max(rr - x, x - (w - rr))
  const qy = Math.max(rr - y, y - (h - rr))
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - rr < 0
}

/** Does a cell at this density survive the ordered-dither threshold? */
export function passesBayer(density: number, cx: number, cy: number): boolean {
  return density > (BAYER[(cy & 7) * 8 + (cx & 7)]! + 0.5) / 64
}

/**
 * The border radius to mask with. `rounded-full` computes to a value
 * `parseFloat` cannot read; treat anything unreadable as "as round as it can
 * be" and let `insideRounded` clamp it.
 */
export function maskRadius(computed: string, w: number, h: number): number {
  const r = parseFloat(computed)
  return Number.isFinite(r) ? r : Math.max(w, h)
}

/* ── One element's field ───────────────────────────────────────────────── */

class Field {
  private ctx: CanvasRenderingContext2D | null

  /** Element box in CSS px. */
  private w = 0
  private h = 0
  private radius = 0
  /** Sub-pixel offset onto the page-wide lattice, and its cell origin. */
  private fx = 0
  private fy = 0
  private ox = 0
  private oy = 0

  /** 0 = full texture, 1 = solid. See `solidity`. */
  private solid = 0

  visible = true
  private painted = -1

  constructor(
    readonly host: HTMLElement,
    readonly canvas: HTMLCanvasElement,
  ) {
    this.ctx = canvas.getContext('2d')
  }

  /** Re-read geometry, including where this element sits on the page lattice
   *  (`latticeOrigin` explains why that is captured here and not per frame). */
  measure(): void {
    const rect = this.host.getBoundingClientRect()
    const dpr = window.devicePixelRatio || 1
    this.w = rect.width
    this.h = rect.height
    if (this.w <= 0 || this.h <= 0) return

    this.radius = maskRadius(getComputedStyle(this.host).borderTopLeftRadius, this.w, this.h)

    const ox = latticeOrigin(rect.left + window.scrollX, PITCH)
    const oy = latticeOrigin(rect.top + window.scrollY, PITCH)
    this.fx = ox.frac
    this.ox = ox.cell
    this.fy = oy.frac
    this.oy = oy.cell

    let covered = 0
    this.forEachCell(() => covered++)
    this.solid = solidity(covered)

    this.canvas.width = Math.round(this.w * dpr)
    this.canvas.height = Math.round(this.h * dpr)
    this.ctx?.setTransform(dpr, 0, 0, dpr, 0, 0)
    this.painted = -1
  }

  /** Walk every cell of the page lattice that this element's mask covers,
   *  in local px and in page-cell coords. Measure counts them; paint fills. */
  private forEachCell(fn: (x: number, y: number, gx: number, gy: number) => void): void {
    const cols = Math.ceil((this.w + this.fx) / PITCH)
    const rows = Math.ceil((this.h + this.fy) / PITCH)
    for (let cy = 0; cy < rows; cy++) {
      const y = cy * PITCH - this.fy + PITCH / 2
      for (let cx = 0; cx < cols; cx++) {
        const x = cx * PITCH - this.fx + PITCH / 2
        if (!insideRounded(x, y, this.w, this.h, this.radius)) continue
        fn(x, y, this.ox + cx, this.oy + cy)
      }
    }
  }

  paint(bucket: number, color: RGB): void {
    const ctx = this.ctx
    if (!ctx || this.w <= 0 || this.h <= 0 || bucket === this.painted) return
    this.painted = bucket

    ctx.clearRect(0, 0, this.w, this.h)
    const [r, g, b] = color
    const off = (PITCH - DOT) / 2

    // `gx`/`gy` are PAGE-space cell coords: one lattice and one noise field for
    // the whole document, sampled through each element's mask.
    this.forEachCell((x, y, gx, gy) => {
      const noise = staticDensity(gx, gy, bucket)
      const density = noise + (1 - noise) * this.solid
      if (!passesBayer(density, gx, gy)) return

      const alpha = ALPHA_FLOOR + (MAX_ALPHA - ALPHA_FLOOR) * clampUnit(density)
      ctx.fillStyle = `rgba(${r},${g},${b},${alpha})`
      ctx.fillRect(x - PITCH / 2 + off, y - PITCH / 2 + off, DOT, DOT)
    })
  }

  invalidate(): void {
    this.painted = -1
  }
}

/* ── The shared loop ───────────────────────────────────────────────────── */

const fields = new Set<Field>()
const byHost = new WeakMap<Element, Field>()

let color: RGB | null = null
let reduced = false
let resizeObs: ResizeObserver | null = null
let viewObs: IntersectionObserver | null = null
let offTheme: (() => void) | null = null
let offMotion: (() => void) | null = null
let offClock: (() => void) | null = null

/**
 * The 8Hz tick every field is currently painted at.
 *
 * Also what any OUT-OF-BAND repaint uses — a resize, a theme flip, a skeleton
 * mounting between ticks. Without that, a field that appeared just after a tick
 * would stay blank for up to 125ms, which is long enough to see.
 */
let bucket = 0

function currentColor(): RGB {
  color ??= parseColor(
    getComputedStyle(document.documentElement).getPropertyValue('--theme-text') || '#808080',
  )
  return color
}

function paintAll(): void {
  const c = currentColor()
  for (const f of fields) if (f.visible) f.paint(bucket, c)
}

/**
 * Ride the page clock while there is something to animate.
 *
 * The clock is `lib/waiting`'s — ONE rAF loop for the whole app, shared with
 * the agent waiting marks, so a cockpit full of skeletons and marks wakes the
 * compositor on one schedule instead of two. This engine only buckets on top
 * of it: static re-rolls at 8Hz and the frames in between are deliberately
 * skipped, so the field flickers rather than boils.
 *
 * (The clock lives under `waiting/` because that is where it was written; it
 * is general, and moving it somewhere neutral is a conversation with its owner
 * rather than something to do behind them.)
 */
function syncClock(): void {
  const wanted = !reduced && fields.size > 0
  if (wanted === !!offClock) return
  if (wanted) {
    offClock = subscribeToClock((elapsed) => {
      const next = tickBucket(elapsed)
      if (next === bucket) return
      bucket = next
      paintAll()
    })
  } else {
    offClock?.()
    offClock = null
  }
}

function onReducedChange(next: boolean): void {
  reduced = next
  syncClock()
  // Freezing leaves the field on whatever roll of the noise it reached, which
  // is the same material standing still — not a flattened mean.
  if (reduced) {
    for (const f of fields) f.invalidate()
    paintAll()
  }
}

function startObservers(): void {
  resizeObs = new ResizeObserver((entries) => {
    for (const e of entries) {
      const f = byHost.get(e.target)
      if (!f) continue
      f.measure()
      f.paint(bucket, currentColor())
    }
  })

  viewObs = new IntersectionObserver((entries) => {
    for (const e of entries) {
      const f = byHost.get(e.target)
      if (!f) continue
      f.visible = e.isIntersecting
      if (f.visible) f.paint(bucket, currentColor())
    }
  })

  // The canvas cannot inherit a CSS variable the way DOM paint does, so a
  // theme flip has to be repainted — otherwise dark-theme dots sit on a
  // paper-white surface until the next tick. The observer behind this is
  // shared with every other canvas field on the page.
  offTheme = onThemeChange(() => {
    color = null
    for (const f of fields) f.invalidate()
    paintAll()
  })

  // Fires immediately with the current preference, so `reduced` is correct
  // before the first field measures.
  offMotion = onReducedMotion(onReducedChange)
}

function stopObservers(): void {
  resizeObs?.disconnect()
  viewObs?.disconnect()
  offTheme?.()
  offMotion?.()
  offClock?.()
  resizeObs = null
  viewObs = null
  offTheme = offMotion = offClock = null
  color = null
}

/**
 * Register one skeleton element and its canvas with the shared field. Returns
 * the teardown — the observers and the clock subscription exist only while at
 * least one skeleton is mounted, which is most of the time none.
 */
export function attachSkeletonField(host: HTMLElement, canvas: HTMLCanvasElement): () => void {
  const field = new Field(host, canvas)
  if (fields.size === 0) startObservers()
  fields.add(field)
  byHost.set(host, field)
  resizeObs?.observe(host)
  viewObs?.observe(host)

  field.measure()
  field.paint(bucket, currentColor())
  syncClock()

  return () => {
    resizeObs?.unobserve(host)
    viewObs?.unobserve(host)
    byHost.delete(host)
    fields.delete(field)
    if (fields.size === 0) stopObservers()
    else syncClock()
  }
}

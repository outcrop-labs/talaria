/**
 * THE DITHER STATE SURFACE — a control's field, painted per cell.
 *
 * Adapted from dither-kit (MIT, Boring-Software-Inc/dither-kit), whose button
 * does the thing every CSS attempt here could not.
 *
 * WHY THIS EXISTS, and it is worth stating plainly because a masked tile got
 * very close. A CSS mask over a dot lattice is BINARY at the boundary: a cell
 * is inside the mask or outside it, with no state in between. So anything that
 * moves — a band growing, a window opening, a reveal spreading — switches whole
 * ranks of dots on at once, and every one of those is an event the eye catches.
 * Six different animations were tried and each failed in its own way for the
 * same underlying reason. It is not a tuning problem and no easing curve
 * reaches it.
 *
 * Painting the cells directly removes the boundary entirely. There is nothing
 * to cross a lattice, because the lattice IS the pixels: every cell is redrawn
 * every frame at whatever alpha the current state calls for, so a change of
 * state is 1,400 simultaneous fades rather than a shape moving over a stencil.
 *
 * THE CANVAS IS TINY, which is what makes this affordable at all — the trick
 * this borrows wholesale. One canvas pixel per dither CELL, so a 184x30 row is
 * a 92x15 canvas: about 1,400 pixels, redrawn only while a state is settling
 * and then never again. `image-rendering: pixelated` scales it up with no
 * resampling, so the dots stay exactly as crisp as an SVG tile's.
 */

/** CSS px per dither cell. The house lattice — see `lib/dither.ts`. */
export const CELL = 2

/**
 * Alpha of an unlit cell relative to a lit one — dither-kit's `OFF_TIER`.
 *
 * The field modulates between two tiers of one colour rather than leaving
 * holes. It reads denser at the same pitch, and a cell crossing the threshold
 * steps between two near values instead of appearing out of nothing.
 */
export const OFF_TIER = 0.4

/** 4x4 ordered matrix, normalised to thresholds — dither-kit's, not the 8x8. */
const BAYER4 = [
  [0, 8, 2, 10],
  [12, 4, 14, 6],
  [3, 11, 1, 9],
  [15, 7, 13, 5],
].map((row) => row.map((v) => (v + 0.5) / 16))

const clamp01 = (v: number): number => (v < 0 ? 0 : v > 1 ? 1 : v)

/**
 * Stable per-cell value, for scattering the threshold.
 *
 * An ordered matrix at CONSTANT density returns the matrix itself — a 4x4 grid
 * you can read off the screen, which is what a flat hover fill is. Mixing a
 * hash into the threshold keeps the even spacing that makes ordered dithering
 * look deliberate and loses the repeat that makes it look like mesh. Gradients
 * keep the matrix's ordering where it earns its keep.
 */
function hash01(x: number, y: number): number {
  let h = (x * 374761393 + y * 668265263) | 0
  h = (h ^ (h >>> 13)) * 1274126177
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296
}

/** How much per-cell randomness is mixed into the ordered threshold. */
const SCATTER = 0.38

export interface DitherSurfaceOptions {
  /** Interior density, 0..1 — how many cells the fill lights. */
  density?: number
  /** Weight of the interior fill against the band, 0..1. */
  weight?: number
  /** How far the field reaches past the control, in CSS px, for the band. */
  band?: number
  /** Is this control the selected one? Drives the accent band. */
  selected?: () => boolean
  /** Does the interior fill show at rest (a selected tile) or only on approach? */
  always?: () => boolean
  /**
   * Which token the fill is drawn in.
   *
   * `surface` is the hairline tone — right where the field is one signal among
   * several, and deliberately close to what it sits on. `text` is the reading
   * tone, which has room to move in BOTH themes: on a segmented cell the field
   * is the entire statement, and the hairline tone could not carry it — it is
   * only about forty levels from the tile it sits on, so even at full coverage
   * the cell lifted five.
   */
  ink?: 'surface' | 'text'
}

interface Tone {
  fill: [number, number, number]
  text: [number, number, number]
  accent: [number, number, number]
}

function readTone(el: HTMLElement): Tone {
  const s = getComputedStyle(el)
  const parse = (v: string): [number, number, number] => {
    const m = /(-?[\d.]+)[,\s]+(-?[\d.]+)[,\s]+(-?[\d.]+)/.exec(v)
    return m ? [+m[1]!, +m[2]!, +m[3]!] : [128, 128, 128]
  }
  // Resolved through a throwaway element so tokens in any colour syntax come
  // back as rgb() — the canvas cannot read a CSS variable.
  const probe = document.createElement('span')
  probe.style.display = 'none'
  el.appendChild(probe)
  probe.style.color = s.getPropertyValue('--theme-border-strong') || '#4a4640'
  const fill = parse(getComputedStyle(probe).color)
  probe.style.color = s.getPropertyValue('--theme-text') || '#e7e2db'
  const text = parse(getComputedStyle(probe).color)
  probe.style.color = s.getPropertyValue('--theme-accent') || '#c8a45c'
  const accent = parse(getComputedStyle(probe).color)
  probe.remove()
  return { fill, text, accent }
}

/**
 * Paint one frame.
 *
 * `intensity` is the whole animation: 0 at rest, 1 on approach. It lowers the
 * dither threshold slightly and lifts alpha — dither-kit's two levers — so the
 * field thickens and brightens together without anything moving.
 */
function paint(
  ctx: CanvasRenderingContext2D,
  cols: number,
  rows: number,
  pad: number,
  radius: number,
  tone: Tone,
  density: number,
  weight: number,
  ink: 'surface' | 'text',
  intensity: number,
  selection: number,
  dpr: number,
): void {
  ctx.clearRect(0, 0, cols * CELL * dpr, rows * CELL * dpr)
  const dot = Math.max(1, Math.round(dpr))
  const [fr, fg, fb] = ink === 'text' ? tone.text : tone.fill
  const [ar, ag, ab] = tone.accent

  // THE CONTROL'S SHAPE, IN CELLS. A canvas is a rectangle and the control is
  // not; without this the band squares off at the corners, which is the first
  // place the eye checks whether a treatment belongs to the thing it edges.
  const hw = (cols - pad * 2) / 2
  const hh = (rows - pad * 2) / 2
  const r = Math.min(radius / CELL, hw, hh)
  const cx = cols / 2
  const cy = rows / 2

  /** Signed distance to the control's rounded rect: negative inside. */
  const sdf = (x: number, y: number): number => {
    const qx = Math.abs(x + 0.5 - cx) - hw + r
    const qy = Math.abs(y + 0.5 - cy) - hh + r
    const ox = Math.max(qx, 0)
    const oy = Math.max(qy, 0)
    return Math.hypot(ox, oy) + Math.min(Math.max(qx, qy), 0) - r
  }

  // ONE DOT PER CELL, NOT ONE BLOCK PER CELL. dither-kit paints a canvas pixel
  // per cell and scales it up, which gives its charts their deliberately chunky
  // look. The house texture is finer — a 1px dot on a 2px lattice, with a gap —
  // so the canvas runs at device resolution and the dot is drawn inside the
  // cell. Same lattice as the CSS tile and `lib/dither.ts`, so a canvas field
  // and a masked one are the same material.
  for (let y = 0; y < rows; y++) {
    for (let x = 0; x < cols; x++) {
      const sd = sdf(x, y)
      let d: number
      let colour: [number, number, number]

      if (sd >= 0) {
        // THE BAND — outside the control, densest against its edge, thinning
        // outward. It follows the rounded shape because the distance does.
        if (selection <= 0.002 || sd > pad) continue
        d = selection * 0.62 * (1 - clamp01(sd / pad)) ** 1.4
        colour = [ar, ag, ab]
      } else {
        // THE FILL — weighted toward the centre, so it emerges from the middle
        // as intensity rises rather than arriving everywhere at once.
        const nx = (x + 0.5 - cx) / Math.max(1, hw)
        const ny = (y + 0.5 - cy) / Math.max(1, hh)
        d = density * intensity * clamp01(1.15 - Math.hypot(nx, ny) * 0.8)
        colour = [fr, fg, fb]
      }
      if (d <= 0.002) continue

      const ordered = BAYER4[y & 3]![x & 3]!
      const threshold = ordered + (hash01(x, y) - ordered) * SCATTER - 0.1 * intensity
      const lit = d > threshold
      const k = (0.3 + d * 0.7) * (1 + 0.22 * intensity)
      // THE FILL SITS BEHIND THE BAND, NOT BESIDE IT. The band is the
      // statement — it is what says "this one" — and an interior at the same
      // weight competes with it and with the label over it. `weight` holds the
      // fill back so it reads as the surface having material rather than as a
      // second mark.
      const alpha = clamp01((lit ? k : k * OFF_TIER) * (sd >= 0 ? 1 : weight))
      if (alpha <= 0.004) continue
      ctx.fillStyle = `rgba(${colour[0]},${colour[1]},${colour[2]},${alpha})`
      ctx.fillRect(x * CELL * dpr, y * CELL * dpr, dot, dot)
    }
  }
}

/**
 * Attach a dither field to a control.
 *
 * Svelte attachment: `<button {@attach ditherSurface()}>`. It inserts its own
 * canvas, watches the control's own pointer and focus, and cleans up after
 * itself. The canvas sits at `z-index: -1` inside the control — above its
 * background and below its content, which is where a `::before` sat and is the
 * only position that works for a fill.
 */
export function ditherSurface(opts: DitherSurfaceOptions = {}) {
  return (node: HTMLElement): (() => void) => {
    const pad = Math.round((opts.band ?? 0) / CELL)
    const density = opts.density ?? 0.62
    const weight = opts.weight ?? 0.55
    const ink = opts.ink ?? 'surface'

    const canvas = document.createElement('canvas')
    canvas.setAttribute('aria-hidden', 'true')
    canvas.style.cssText =
      `position:absolute;inset:${-pad * CELL}px;width:calc(100% + ${pad * CELL * 2}px);` +
      `height:calc(100% + ${pad * CELL * 2}px);z-index:-1;pointer-events:none`
    const ctx = canvas.getContext('2d')
    if (!ctx) return () => {}

    // The control has to establish a stacking context, or a negative z-index
    // escapes behind an ancestor's background instead of sitting behind its
    // own content.
    const prev = { position: node.style.position, isolation: node.style.isolation }
    if (getComputedStyle(node).position === 'static') node.style.position = 'relative'
    node.style.isolation = 'isolate'
    node.insertBefore(canvas, node.firstChild)

    let tone = readTone(node)
    let radius = 0
    let dpr = window.devicePixelRatio || 1
    let cols = 0
    let rows = 0
    let intensity = 0
    let selection = 0
    let raf = 0
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches

    const render = () =>
      paint(ctx, cols, rows, pad, radius, tone, density, weight, ink, intensity, selection, dpr)

    const targets = () => ({
      intensity: hot || opts.always?.() ? 1 : 0,
      selection: opts.selected?.() ? 1 : 0,
    })

    /**
     * The whole animation, and it is four lines on purpose — dither-kit's
     * exponential ease. It converges without a duration or a curve to pick,
     * every cell moves continuously because the field is recomputed rather
     * than revealed, and the loop stops the moment it has arrived.
     */
    const tick = () => {
      const t = targets()
      const di = t.intensity - intensity
      const ds = t.selection - selection
      if (Math.abs(di) < 0.01 && Math.abs(ds) < 0.01) {
        intensity = t.intensity
        selection = t.selection
        render()
        raf = 0
        return
      }
      intensity += di * 0.16
      selection += ds * 0.12
      render()
      raf = requestAnimationFrame(tick)
    }

    const settle = () => {
      if (reduced) {
        const t = targets()
        intensity = t.intensity
        selection = t.selection
        render()
      } else if (!raf) {
        raf = requestAnimationFrame(tick)
      }
    }

    let hot = false
    const enter = () => ((hot = true), settle())
    const leave = () => ((hot = false), settle())
    node.addEventListener('pointerenter', enter)
    node.addEventListener('pointerleave', leave)
    node.addEventListener('focusin', enter)
    node.addEventListener('focusout', leave)

    const resize = () => {
      // `offsetWidth` / `offsetHeight`, NOT `getBoundingClientRect`. The mark
      // this attaches to is mid-crossfade when it mounts, and a bounding rect
      // is measured THROUGH that transform — so the first reading is a scaled
      // fraction of the real size, and since the element's layout size never
      // actually changes, the ResizeObserver has nothing to correct it with.
      // The field stayed a small blob in the corner for the rest of its life.
      radius = parseFloat(getComputedStyle(node).borderTopLeftRadius) || 0
      const c = Math.max(4, Math.round(node.offsetWidth / CELL) + pad * 2)
      const w = Math.max(4, Math.round(node.offsetHeight / CELL) + pad * 2)
      dpr = window.devicePixelRatio || 1
      if (c === cols && w === rows && canvas.width === cols * CELL * dpr) return
      cols = c
      rows = w
      canvas.width = Math.round(cols * CELL * dpr)
      canvas.height = Math.round(rows * CELL * dpr)
      render()
    }
    resize()
    // A control that is ALREADY selected has no event coming to wake it, so it
    // would sit at intensity 0 and paint nothing at all.
    settle()
    const ro = new ResizeObserver(resize)
    ro.observe(node)

    // A theme flip changes the tokens under a canvas that cannot inherit them.
    const mo = new MutationObserver(() => {
      tone = readTone(node)
      render()
    })
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-theme'] })

    // A control can become the selected one with no pointer anywhere near it —
    // a route change, a keyboard move. Without this the field would sit at its
    // old target until something happened to touch it.
    const so = new MutationObserver(settle)
    so.observe(node, {
      attributes: true,
      attributeFilter: ['data-status', 'data-active', 'aria-selected', 'aria-current', 'aria-pressed', 'class'],
    })

    return () => {
      if (raf) cancelAnimationFrame(raf)
      node.removeEventListener('pointerenter', enter)
      node.removeEventListener('pointerleave', leave)
      node.removeEventListener('focusin', enter)
      node.removeEventListener('focusout', leave)
      ro.disconnect()
      mo.disconnect()
      so.disconnect()
      canvas.remove()
      node.style.position = prev.position
      node.style.isolation = prev.isolation
    }
  }
}

/** The markup signals that mean "this control is the chosen one". */
const SELECTED = [
  '[data-status="active"]',
  '[data-active="true"]',
  '[aria-current="true"]',
  '[aria-current="page"]',
  '[aria-selected="true"]',
  '[aria-pressed="true"]',
].join(',')

const isSelected = (el: HTMLElement): boolean => el.matches(SELECTED)

/**
 * UPGRADE EVERY MARKED CONTROL ON THE PAGE, once, from one place.
 *
 * The three classes stay in the markup as they always were — they are the
 * statement of intent, and 127 call sites already make it correctly. What
 * changed is who honours them: the stylesheet used to paint a masked
 * pseudo-element, and now this hands each element a painted field instead.
 * Doing it here rather than at the call sites means the two renderings could
 * never drift apart, and that switching back is one function rather than a
 * sweep.
 *
 *   dither-fill   a field on approach
 *   dither-bloom  a field on approach, plus the accent band when selected
 *   dither-mark   both, permanently — for an element that IS the mark
 */
export function upgradeDitherSurfaces(root: HTMLElement): () => void {
  const attached = new WeakMap<HTMLElement, () => void>()

  const optionsFor = (el: HTMLElement): DitherSurfaceOptions | null => {
    // `data-dither-band="0"` — the field without the accent outline.
    //
    // A switch is not a destination. The band says "this is the one you are
    // on", which is right for a nav row or a tab and wrong for a segmented
    // cell or a toggle: those are settings, and outlining one in the accent
    // gives a preference the weight of a location.
    const quiet = el.dataset.ditherBand === '0'
    // A QUIET FIELD CARRIES THE WHOLE STATEMENT, so it cannot be quiet in the
    // way a nav row's is. There the fill sits behind a band and a raised tile
    // and is deliberately held back — it is the third thing saying "this one".
    // On a segmented cell it is the ONLY thing, with nothing beside it to
    // carry the meaning, and at the same weight it was barely visible in
    // either theme. Full weight and a denser field.
    const QUIET = { density: 0.72, weight: 0.78, ink: 'text' as const }

    if (el.classList.contains('dither-mark')) {
      return quiet
        ? { ...QUIET, always: () => true }
        : { band: 6, always: () => true, selected: () => true }
    }
    if (el.classList.contains('dither-bloom')) {
      return quiet
        ? { ...QUIET, always: () => isSelected(el) }
        : { band: 6, always: () => isSelected(el), selected: () => isSelected(el) }
    }
    if (el.classList.contains('dither-fill')) return {}
    return null
  }

  const scan = () => {
    for (const el of root.querySelectorAll<HTMLElement>('.dither-fill,.dither-bloom,.dither-mark')) {
      if (attached.has(el)) continue
      const opts = optionsFor(el)
      if (!opts) continue
      attached.set(el, ditherSurface(opts)(el))
    }
  }

  scan()
  // Only childList: an attribute change on a marked element is the control
  // reporting a new state, which its own surface already watches. Reacting to
  // it here would tear the surface down and build it again mid-hover.
  const mo = new MutationObserver((records) => {
    if (records.some((r) => r.addedNodes.length > 0)) scan()
    for (const r of records) {
      for (const n of r.removedNodes) {
        if (!(n instanceof HTMLElement)) continue
        const stop = attached.get(n)
        if (stop) {
          stop()
          attached.delete(n)
        }
      }
    }
  })
  mo.observe(root, { childList: true, subtree: true })

  return () => mo.disconnect()
}

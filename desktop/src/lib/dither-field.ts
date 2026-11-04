/**
 * The launcher's ambient field, painted ONCE as pixels.
 *
 * The launcher used to mount ui's live DitherEngine over the whole window
 * with a shimmer. At the house grain (pitch 2 / dot 1) a 1280×769 window is
 * about 246 000 cells, and the uniform grain puts nearly every one of them in
 * the shimmer's jitter band, so each 160 ms tick re-rolled about half the
 * field as individual 1-px clearRect + fillRect pairs: ~105 000 fills and
 * ~110 000 clears per tick. WebKitGTK spent ~0.5 s of the web process's main
 * thread on every one of those frames (97 % CPU, rAF every ~500 ms), and every
 * click, keystroke and IPC reply waited behind them — the "slow and
 * unresponsive" shell. A rebuild (first paint, resize, return to the launcher)
 * cost 310–530 ms of script on top.
 *
 * This keeps the material and drops the cost:
 *   - the SAME field: ui's own `evalSource`, Bayer matrix, two-tier rule,
 *     organic clump hash and page lattice, read from `@dither` (the engine
 *     file imports nothing, which is what makes it shareable);
 *   - STATIC: no shimmer, so nothing runs while the launcher sits idle;
 *   - one `putImageData` per paint instead of a quarter of a million path
 *     operations, so a paint is a few tens of milliseconds even on a
 *     software rasterizer, and only happens on mount, resize or a DPR change.
 *
 * The product UI keeps the live engine; only the launcher's full-window case
 * is static.
 */

import {
  BAYER,
  OFF_TIER,
  evalSource,
  hash01,
  latticeOrigin,
  parseColor,
  type DitherSource,
  type DitherTone,
  type RGB,
} from '@dither'

export interface FieldOptions {
  /** Grid pitch in CSS px (the house grain is 2). */
  pitch?: number
  /** Dot size in CSS px, <= pitch (the house grain is 1). */
  dot?: number
  alphaFloor?: number
  maxAlpha?: number
  /** 0..1 static clump noise, as in the engine. */
  organic?: number
}

const TONE_VAR: Record<DitherTone, string> = {
  neutral: '--theme-text',
  accent: '--theme-accent',
  success: '--theme-success',
  danger: '--theme-danger',
  surface: '--theme-panel',
}

const clamp01 = (x: number) => Math.min(1, Math.max(0, x))

const LITTLE_ENDIAN = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1

/** RGBA bytes as one Uint32 in the platform's byte order. */
const pack = (r: number, g: number, b: number, a: number) =>
  (LITTLE_ENDIAN ? (a << 24) | (b << 16) | (g << 8) | r : (r << 24) | (g << 16) | (b << 8) | a) >>> 0

function resolveTones(): Record<DitherTone, RGB> {
  const style = getComputedStyle(document.documentElement)
  const out = {} as Record<DitherTone, RGB>
  for (const tone of Object.keys(TONE_VAR) as DitherTone[]) {
    out[tone] = parseColor(style.getPropertyValue(TONE_VAR[tone]) || '#808080')
  }
  return out
}

interface Sample {
  miss: number
  r: number
  g: number
  b: number
  wsum: number
}

/** Screen-accumulate density at a point; colour is the tone mix weighted by
 * each source's contribution (the engine's rule, verbatim). Null: no ink. */
function sample(
  active: DitherSource[],
  tones: Record<DitherTone, RGB>,
  x: number,
  y: number,
  w: number,
  h: number,
): Sample | null {
  let miss = 1
  let r = 0
  let g = 0
  let b = 0
  let wsum = 0
  for (const s of active) {
    const v = evalSource(s, x, y, 0, w, h)
    if (v <= 0) continue
    miss *= 1 - clamp01(v)
    const c = tones[s.tone ?? 'neutral']
    r += c[0] * v
    g += c[1] * v
    b += c[2] * v
    wsum += v
  }
  return wsum > 0 ? { miss, r, g, b, wsum } : null
}

const dependsOnYOnly = (s: DitherSource) =>
  s.kind === 'uniform' ||
  (s.kind === 'edge' && (s.side === 'top' || s.side === 'bottom')) ||
  ((s.kind === 'ramp' || s.kind === 'wave') && s.axis === 'y')

/**
 * Paint `sources` into `canvas` at `wCss`×`hCss` CSS px and `dpr`. Returns
 * the milliseconds it took (the rig reads it; nothing else needs it).
 */
export function paintField(
  canvas: HTMLCanvasElement,
  sources: DitherSource[],
  wCss: number,
  hCss: number,
  dpr: number,
  opts: FieldOptions = {},
): number {
  const t0 = performance.now()
  const pitch = opts.pitch ?? 2
  const dot = opts.dot ?? 1
  const alphaFloor = opts.alphaFloor ?? 0.18
  const maxAlpha = opts.maxAlpha ?? 0.55
  const organic = opts.organic ?? 0

  const W = Math.max(1, Math.round(wCss * dpr))
  const H = Math.max(1, Math.round(hCss * dpr))
  canvas.width = W
  canvas.height = H
  const ctx = canvas.getContext('2d')
  if (!ctx) return 0
  const image = ctx.createImageData(W, H)
  const px = new Uint32Array(image.data.buffer)
  const tones = resolveTones()
  const active = sources.filter((s) => s.strength > 0.002)
  const rowOnly = active.every(dependsOnYOnly)

  // Key to the page-wide lattice, exactly as the engine does, so the field
  // lines up with any other dither on the page.
  const rect = canvas.getBoundingClientRect()
  const gxo = latticeOrigin(rect.left + window.scrollX, pitch)
  const gyo = latticeOrigin(rect.top + window.scrollY, pitch)
  const cols = Math.ceil((wCss + gxo.frac) / pitch)
  const rows = Math.ceil((hCss + gyo.frac) / pitch)
  const off = (pitch - dot) / 2

  // A dot is a CSS-px square at a fractional device position. WebKit's
  // canvas draws the engine's fillRect dots SNAPPED to whole device pixels,
  // not antialiased (measured against the live engine in the same webview),
  // so each dot covers the pixels its rounded edges span: 1×1 at 1×, 2×2 at
  // 2×, 1 or 2 at a fractional ratio. Dots never overlap: the gap
  // (pitch − dot) is at least one device pixel.
  for (let cy = 0; cy < rows; cy++) {
    const y = cy * pitch - gyo.frac + pitch / 2
    const top = cy * pitch - gyo.frac + off
    const y0 = Math.max(0, Math.round(top * dpr))
    const y1 = Math.min(H, Math.round((top + dot) * dpr))
    if (y1 <= y0) continue
    const gy = gyo.cell + cy
    // A row-only field (the launcher's is: two edges and a uniform grain) is
    // the same on every cell of a row, so it is evaluated once per row.
    const rowSample = rowOnly ? sample(active, tones, 0, y, wCss, hCss) : null
    for (let cx = 0; cx < cols; cx++) {
      const x = cx * pitch - gxo.frac + pitch / 2
      const cell = rowSample ?? sample(active, tones, x, y, wCss, hCss)
      if (!cell) continue
      const { miss, r, g, b, wsum } = cell
      const gx = gxo.cell + cx
      let density = 1 - miss
      if (organic > 0) {
        const clump = 0.6 * hash01(gx >> 2, gy >> 2, 7) + 0.4 * hash01(gx, gy, 13)
        density *= 1 + organic * (clump * 1.8 - 0.9)
      }
      if (density <= 0.002) continue
      const threshold = (BAYER[(gy & 7) * 8 + (gx & 7)]! + 0.5) / 64
      const alpha =
        density > threshold
          ? alphaFloor + (maxAlpha - alphaFloor) * clamp01(density)
          : maxAlpha * clamp01(density) * OFF_TIER
      const left = cx * pitch - gxo.frac + off
      const x0 = Math.max(0, Math.round(left * dpr))
      const x1 = Math.min(W, Math.round((left + dot) * dpr))
      if (x1 <= x0) continue
      const value = pack(
        Math.round(r / wsum),
        Math.round(g / wsum),
        Math.round(b / wsum),
        Math.round(alpha * 255),
      )
      for (let py = y0; py < y1; py++) {
        const row = py * W
        for (let pxi = x0; pxi < x1; pxi++) px[row + pxi] = value
      }
    }
  }
  ctx.putImageData(image, 0, 0)
  return performance.now() - t0
}

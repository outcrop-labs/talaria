import assert from 'node:assert/strict'
import { afterEach, beforeEach, test } from 'vitest'
import { DitherEngine, type DitherEngineOptions, type DitherSource } from './dither-engine'

// The engine's frame loop and its incremental paint, against a recording 2D
// context and a hand-driven requestAnimationFrame. dither.test.ts covers the
// field math; this covers what the paint loop promises (N11): a field with
// nothing travelling is painted and then PARKS, so no frame is scheduled
// while nothing on it changes (a speed-0 wave included), and a field that
// moves keeps its loop until reduced motion parks it.

type Call = { op: 'clearRect' | 'fillRect'; x: number; y: number; w: number; h: number }

let frames: Array<(t: number) => void> = []
const saved: Record<string, unknown> = {}

function recordingCanvas(left: number, top: number) {
  const calls: Call[] = []
  const ctx = {
    fillStyle: '',
    setTransform() {},
    save() {},
    restore() {},
    beginPath() {},
    rect() {},
    clip() {},
    clearRect: (x: number, y: number, w: number, h: number) => calls.push({ op: 'clearRect', x, y, w, h }),
    fillRect: (x: number, y: number, w: number, h: number) => calls.push({ op: 'fillRect', x, y, w, h }),
  }
  const canvas = { width: 0, height: 0, getContext: () => ctx, getBoundingClientRect: () => ({ left, top }) }
  return { canvas: canvas as unknown as HTMLCanvasElement, calls }
}

beforeEach(() => {
  frames = []
  for (const k of ['document', 'window', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame', 'queueMicrotask']) {
    saved[k] = (globalThis as Record<string, unknown>)[k]
  }
  const scratch = { fillStyle: '' }
  Object.assign(globalThis, {
    document: {
      hidden: false,
      documentElement: {},
      createElement: () => ({ getContext: () => scratch }),
      addEventListener() {},
      removeEventListener() {},
    },
    window: { scrollX: 0, scrollY: 0 },
    getComputedStyle: () => ({ getPropertyValue: () => '#e0e0e0' }),
    requestAnimationFrame: (cb: (t: number) => void) => frames.push(cb),
    cancelAnimationFrame() {},
  })
})

// Every engine a test makes is destroyed after it, as an unmount does: the
// shared ticker is module state, and an engine left queued would hold the
// frame the next test's engines wait on (a test's `frames = []` drops it).
const made: DitherEngine[] = []

afterEach(() => {
  for (const e of made.splice(0)) e.destroy()
  Object.assign(globalThis, saved)
})

/** Run the queued frames at `step`-ms intervals until none is queued or
 *  `limit` frames ran. Returns how many ran. */
function drive(from: number, step: number, limit: number): number {
  let ran = 0
  let t = from
  while (frames.length > 0 && ran < limit) {
    const next = frames.shift()!
    next(t)
    t += step
    ran++
  }
  return ran
}

function engine(sources: DitherSource[], opts: DitherEngineOptions, at: { left: number; top: number; dpr: number }) {
  const { canvas, calls } = recordingCanvas(at.left, at.top)
  const e = new DitherEngine(canvas, opts)
  made.push(e)
  e.setSize(240, 120, at.dpr)
  e.setSources(sources, { immediate: true })
  return { e, calls, canvas }
}

const edge: DitherSource = { id: 'top', kind: 'edge', side: 'top', depth: 60, strength: 0.4 }

test('a still field is painted once and parks: no frame waits once it is drawn', () => {
  const { calls } = engine([edge], {}, { left: 10, top: 10, dpr: 1 })
  const ran = drive(performance.now() + 1000, 16, 50)
  assert.ok(ran <= 2, `a still field ran ${ran} frames`)
  assert.equal(frames.length, 0)
  assert.ok(calls.some((c) => c.op === 'fillRect'), 'it painted')
})

test('a speed-0 wave is a still field; a travelling one keeps its loop', () => {
  const still = engine([edge, { id: 'w', kind: 'wave', axis: 'x', wavelength: 120, speed: 0, strength: 0.3 }], {}, { left: 10, top: 10, dpr: 1 })
  drive(performance.now() + 1000, 16, 50)
  assert.equal(frames.length, 0, 'the speed-0 wave parked')
  assert.ok(still.calls.some((c) => c.op === 'fillRect'))

  frames = []
  engine([edge, { id: 'w', kind: 'wave', axis: 'x', wavelength: 120, speed: 12, strength: 0.3 }], {}, { left: 10, top: 10, dpr: 1 })
  const ran = drive(performance.now() + 1000, 16, 50)
  assert.equal(ran, 50, 'the travelling wave kept scheduling')
  assert.equal(frames.length, 1)
})

test('a shimmer field keeps its loop, and reduced motion parks it', () => {
  const { e } = engine([edge], { shimmer: 0.1 }, { left: 10, top: 10, dpr: 1 })
  assert.equal(drive(performance.now() + 1000, 16, 30), 30)
  e.setReducedMotion(true)
  drive(performance.now() + 2000, 16, 30)
  assert.equal(frames.length, 0)
})

// UB5: the shared ticker. Every live engine used to run its own rAF loop, so
// a board of working cards woke the page once per field per frame.
const drift: DitherSource = { id: 'w', kind: 'wave', axis: 'x', wavelength: 200, speed: 12, strength: 0.3 }

test('every live engine on the page rides one frame request', () => {
  const live = [0, 1, 2, 3].map((i) => engine([edge, { ...drift, id: `w${i}` }], { shimmer: i % 2 ? 0.1 : 0 }, { left: 10, top: 10 + 130 * i, dpr: 1 }))
  // Four engines asked for a frame on mount, and asked once between them.
  assert.equal(frames.length, 1)
  const t0 = performance.now() + 1000
  for (let f = 0; f < 60; f++) {
    frames.shift()!(t0 + f * 16)
    assert.equal(frames.length, 1, `frame ${f} left ${frames.length} requests`)
  }
  for (const { calls } of live) assert.ok(calls.some((c) => c.op === 'fillRect'), 'every field painted')
})

test('an engine beside others paints exactly what it paints alone, frame by frame', () => {
  const at = { left: 11, top: 7, dpr: 1 }
  const sources = [edge, drift]
  const t0 = performance.now() + 1000
  // One string per frame: what the subject drew in it. Compared as strings so
  // a regression names its first differing frame instead of asking assert to
  // diff 90 frames of calls, which takes minutes.
  const run = (company: number): string[] => {
    frames = []
    const subject = engine(sources, { shimmer: 0.1 }, at)
    for (let i = 0; i < company; i++) engine([{ ...drift, id: `c${i}`, speed: 7 + i }], { shimmer: 0.05 }, { left: 300, top: 40 * i, dpr: 1 })
    const perFrame: string[] = []
    for (let f = 0; f < 90; f++) {
      const before = subject.calls.length
      const next = frames.shift()
      if (!next) break
      next(t0 + f * 16.667)
      perFrame.push(subject.calls.slice(before).map((c) => `${c.op}:${c.x},${c.y},${c.w},${c.h}`).join(' '))
    }
    for (const e of made.splice(0)) e.destroy()
    return perFrame
  }
  const alone = run(0)
  const beside = run(5)
  assert.equal(alone.length, 90)
  assert.ok(alone.some((f) => f !== '') && alone.some((f) => f === ''), 'it painted on bucket frames only')
  const first = alone.findIndex((f, i) => beside[i] !== f)
  assert.equal(first, -1, `frame ${first} differs beside other engines`)
  assert.equal(beside.length, alone.length)
})

test('the last engine out cancels the frame it was waiting on', () => {
  const cancelled: number[] = []
  Object.assign(globalThis, { cancelAnimationFrame: (h: number) => cancelled.push(h) })
  const a = engine([edge, drift], {}, { left: 10, top: 10, dpr: 1 })
  const b = engine([edge, { ...drift, id: 'w2' }], {}, { left: 10, top: 200, dpr: 1 })
  assert.equal(frames.length, 1)
  a.e.destroy()
  assert.deepEqual(cancelled, [], 'b still waits on it')
  b.e.destroy()
  assert.equal(cancelled.length, 1)
  // A new engine asks for a fresh frame.
  engine([edge, drift], {}, { left: 10, top: 10, dpr: 1 })
  assert.equal(frames.length, 2)
})

test('a field that throws stops only itself: the rest keep painting, and its error is still reported', () => {
  const reported: unknown[] = []
  Object.assign(globalThis, { queueMicrotask: (cb: () => void) => { try { cb() } catch (err) { reported.push(err) } } })
  const bad = engine([edge, drift], {}, { left: 10, top: 10, dpr: 1 })
  const good = engine([edge, { ...drift, id: 'w2' }], {}, { left: 10, top: 200, dpr: 1 })
  const boom = new Error('boom')
  const ctx = bad.canvas.getContext('2d') as unknown as { fillRect: () => void }
  ctx.fillRect = () => {
    throw boom
  }
  assert.equal(frames.length, 1)
  const t0 = performance.now() + 1000
  frames.shift()!(t0)
  assert.deepEqual(reported, [boom])
  // The one after it in the queue painted, and asked for the next frame.
  assert.ok(good.calls.some((c) => c.op === 'fillRect'))
  assert.equal(frames.length, 1)
  const painted = good.calls.length
  for (let f = 1; f <= 30 && frames.length > 0; f++) frames.shift()!(t0 + f * 16)
  assert.ok(good.calls.length > painted, 'the field after the thrower kept its loop')
  assert.equal(reported.length, 1, 'the thrower never ran again: it stopped itself')
})

// N11 (round 7): the incremental paint leaves exactly what a full rebuild of
// the same frame paints, so a live field cannot build up. Checked against a
// model canvas that rasterises the way the two engines do: a fill covers each
// device pixel by its area (antialiased, source-over), and a clear either
// rounds its rect to whole pixels and erases those (Chromium) or erases each
// pixel by its coverage (WebKit). The engine's old clear — its own fractional
// rect — leaves the half-covered edge pixels of every rewritten dot holding
// ink, which grows with every rewrite; this fails on it.
type Rect = [number, number, number, number]

function rasterCanvas(left: number, top: number, clearMode: 'round' | 'coverage') {
  let scale = 1
  let clip: Rect[] | null = null
  let path: Rect[] = []
  const stack: Array<{ scale: number; clip: Rect[] | null }> = []
  let w = 0
  let h = 0
  let alpha = new Float64Array(0)
  const store = { width: 0, height: 0 }
  const ensure = () => {
    if (store.width !== w || store.height !== h) {
      w = store.width
      h = store.height
      alpha = new Float64Array(w * h)
    }
  }
  const overlap = (a0: number, a1: number, b0: number, b1: number) => Math.max(0, Math.min(a1, b1) - Math.max(a0, b0))
  const clipCover = (px: number, py: number) => {
    if (!clip) return 1
    let c = 0
    for (const [x, y, rw, rh] of clip) c += overlap(px, px + 1, x * scale, (x + rw) * scale) * overlap(py, py + 1, y * scale, (y + rh) * scale)
    return Math.min(1, c)
  }
  // 8-bit storage, as the canvas keeps it.
  const q = (v: number) => Math.round(Math.min(1, Math.max(0, v)) * 255) / 255
  const ctx = {
    fillStyle: '',
    setTransform: (a: number) => {
      scale = a
    },
    save: () => stack.push({ scale, clip }),
    restore: () => {
      const s = stack.pop()!
      scale = s.scale
      clip = s.clip
    },
    beginPath: () => {
      path = []
    },
    rect: (x: number, y: number, rw: number, rh: number) => path.push([x, y, rw, rh]),
    clip: () => {
      clip = path
    },
    fillRect: (x: number, y: number, rw: number, rh: number) => {
      ensure()
      const a = Number(/,([\d.]+)\)$/.exec(ctx.fillStyle)?.[1] ?? 1)
      const x0 = x * scale
      const y0 = y * scale
      const x1 = (x + rw) * scale
      const y1 = (y + rh) * scale
      for (let py = Math.max(0, Math.floor(y0)); py < Math.min(h, Math.ceil(y1)); py++) {
        for (let px = Math.max(0, Math.floor(x0)); px < Math.min(w, Math.ceil(x1)); px++) {
          const cov = overlap(px, px + 1, x0, x1) * overlap(py, py + 1, y0, y1) * clipCover(px, py)
          if (cov <= 0) continue
          const i = py * w + px
          alpha[i] = q(a * cov + alpha[i]! * (1 - a * cov))
        }
      }
    },
    clearRect: (x: number, y: number, rw: number, rh: number) => {
      ensure()
      const x0 = x * scale
      const y0 = y * scale
      const x1 = (x + rw) * scale
      const y1 = (y + rh) * scale
      for (let py = Math.max(0, Math.floor(y0)); py < Math.min(h, Math.ceil(y1)); py++) {
        for (let px = Math.max(0, Math.floor(x0)); px < Math.min(w, Math.ceil(x1)); px++) {
          const i = py * w + px
          const cov =
            clearMode === 'round'
              ? overlap(px, px + 1, Math.round(x0), Math.round(x1)) * overlap(py, py + 1, Math.round(y0), Math.round(y1))
              : overlap(px, px + 1, x0, x1) * overlap(py, py + 1, y0, y1)
          alpha[i] = q(alpha[i]! * (1 - cov * clipCover(px, py)))
        }
      }
    },
  }
  const canvas = Object.assign(store, { getContext: () => ctx, getBoundingClientRect: () => ({ left, top }) })
  return {
    canvas: canvas as unknown as HTMLCanvasElement,
    pixels: () => {
      ensure()
      return alpha
    },
  }
}

type Painter = { paint(now: number, live: boolean): void }

/** Run a live field `count` frames, then rebuild the frame it last painted from
 *  nothing, and count the device pixels that differ. */
function buildUp(
  sources: DitherSource[],
  opts: DitherEngineOptions,
  at: { left: number; top: number; dpr: number },
  clearMode: 'round' | 'coverage',
  count: number,
  mask?: Array<{ x: number; y: number; w: number; h: number }>,
): { differ: number; inc: number; full: number } {
  const mountOn = (c: HTMLCanvasElement) => {
    const e = new DitherEngine(c, opts)
    made.push(e)
    e.setSize(180, 60, at.dpr)
    if (mask) e.setMask(mask)
    e.setSources(sources, { immediate: true })
    return e
  }
  const live = rasterCanvas(at.left, at.top, clearMode)
  const e = mountOn(live.canvas) as unknown as Painter
  // The time of the frame now on the canvas: a live field paints only on the
  // frames where its wave's clock moves.
  let painted = 0
  const paint = e.paint.bind(e)
  e.paint = (now, isLive) => {
    painted = now
    paint(now, isLive)
  }
  const t0 = performance.now() + 1000
  for (let f = 0; f < count; f++) {
    const next = frames.shift()
    if (!next) break
    next(t0 + f * 16.667)
  }
  const fresh = rasterCanvas(at.left, at.top, clearMode)
  ;(mountOn(fresh.canvas) as unknown as Painter).paint(painted, true)
  const a = live.pixels()
  const b = fresh.pixels()
  let differ = 0
  let inc = 0
  let full = 0
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) differ++
    inc += a[i]!
    full += b[i]!
  }
  return { differ, inc, full }
}

const SHAPES: Array<{ name: string; sources: DitherSource[]; opts: DitherEngineOptions; mask?: Array<{ x: number; y: number; w: number; h: number }> }> = [
  // A working card's wash, as designed and as shipped (pitch 4, a fractional dot, a uniform grain).
  { name: 'card wash 1.4', opts: { pitch: 4, dot: 1.4, alphaFloor: 0.04, maxAlpha: 0.26 }, sources: [{ id: 'l', kind: 'edge', side: 'top', depth: 34, strength: 0.16 }, { ...drift, strength: 0.3 }] },
  { name: 'card wash (shipped)', opts: { pitch: 4, dot: 1.4, alphaFloor: 0.037, maxAlpha: 0.25 }, sources: [{ id: 'l', kind: 'edge', side: 'top', depth: 34, strength: 0.2 }, { ...drift, strength: 0.375 }, { id: 'g', kind: 'uniform', strength: 0.056 }] },
  // A list strip (pitch 3), the house grain with two crossing waves (generating), a shimmer.
  { name: 'strip', opts: { pitch: 3, dot: 1.2, alphaFloor: 0.06, maxAlpha: 0.4 }, sources: [{ id: 'l', kind: 'edge', side: 'left', depth: 26, strength: 0.4 }, { ...drift, wavelength: 160, strength: 0.5 }] },
  { name: 'strip (shipped)', opts: { pitch: 3, dot: 1.2, alphaFloor: 0.12, maxAlpha: 0.4 }, sources: [{ id: 'l', kind: 'edge', side: 'left', depth: 26, strength: 0.6 }, { ...drift, wavelength: 160, strength: 0.75 }, { id: 'g', kind: 'uniform', strength: 0.112 }] },
  { name: 'generating', opts: { alphaFloor: 0.03, maxAlpha: 0.32 }, sources: [{ id: 'a', kind: 'wave', axis: 'x', wavelength: 150, speed: 26, strength: 0.4 }, { id: 'b', kind: 'wave', axis: 'y', wavelength: 64, speed: -14, strength: 0.22 }] },
  // The stream tail: masked to fractional line fragments, with shimmer.
  { name: 'stream (masked)', opts: { alphaFloor: 0.06, maxAlpha: 0.4, shimmer: 0.15 }, sources: [{ id: 'v', kind: 'uniform', strength: 0.22 }, { ...drift, strength: 0.08 }], mask: [{ x: 40.4, y: 10.2, w: 120.6, h: 21.5 }, { x: 0, y: 33.7, w: 61.3, h: 21.5 }] },
  // Pitch 3 / dot 2: a gap of exactly one device pixel at DPR 1, the disjoint path's edge.
  { name: 'pitch 3 dot 2', opts: { pitch: 3, dot: 2, alphaFloor: 0.11, maxAlpha: 0.9 }, sources: [{ id: 'l', kind: 'edge', side: 'left', depth: 26, strength: 0.4 }, { ...drift, wavelength: 160, strength: 0.5 }] },
  // Dots that touch (cover mode): the full-rebuild path.
  { name: 'cover', opts: { cover: true }, sources: [{ id: 'r', kind: 'ramp', axis: 'x', from: 0, to: 180, fromLevel: 0.1, toLevel: 0.6, strength: 0.6 }, { ...drift, speed: 18 }] },
]

for (const clearMode of ['round', 'coverage'] as const) {
  test(`a live field paints what a rebuild of the same frame paints — ${clearMode === 'round' ? 'Chromium' : 'WebKit'}'s clear`, () => {
    const misses: string[] = []
    for (const shape of SHAPES) {
      for (const dpr of [1, 1.5, 2]) {
        for (const pos of [0, 0.25, 0.5]) {
          frames = []
          const r = buildUp(shape.sources, shape.opts, { left: 30 + pos, top: 20 + pos, dpr }, clearMode, 150, shape.mask)
          if (r.differ > 0) misses.push(`${shape.name} @ dpr ${dpr} +${pos}: ${r.differ} px, ${(r.inc / r.full).toFixed(2)}x the ink`)
          for (const e of made.splice(0)) e.destroy()
        }
      }
    }
    assert.deepEqual(misses, [])
  })
}

test('a page zoomed out (dots closer than a device pixel) rebuilds instead of drifting', () => {
  const shape = SHAPES[3]!
  for (const dpr of [0.67, 0.8, 0.9]) {
    frames = []
    const r = buildUp(shape.sources, shape.opts, { left: 30.25, top: 20.25, dpr }, 'round', 120)
    assert.equal(r.differ, 0, `dpr ${dpr}`)
    for (const e of made.splice(0)) e.destroy()
  }
})

// The cell skip. A ring trace (`edgeBand`) and a mask only stop cells that
// cannot show from being drawn: inside the ring the pixels are the ones a full
// field leaves, and the skip holds from a fade-in mount whose first frame
// lands before anything is drawn (its first frame returns early, and the
// frames after it are not fresh). The clock is pinned so two runs paint the
// same frames: a wave's phase is the absolute time.
let clock = 0
function pinClock(fn: () => void) {
  const saved = Object.getOwnPropertyDescriptor(globalThis, 'performance')!
  Object.defineProperty(globalThis, 'performance', { value: { now: () => clock }, configurable: true, writable: true })
  try {
    fn()
  } finally {
    Object.defineProperty(globalThis, 'performance', saved)
  }
}

function trackPaint(e: DitherEngine) {
  const p = e as unknown as Painter
  const at = { t: 0 }
  const paint = p.paint.bind(p)
  p.paint = (now, isLive) => {
    at.t = now
    paint(now, isLive)
  }
  return at
}

function ringRun(opts: DitherEngineOptions, firstAtMount: boolean) {
  const canvasOf = rasterCanvas(30.25, 20.5, 'round')
  const fills = { n: 0 }
  const ctx = canvasOf.canvas.getContext('2d') as unknown as { fillRect: (...a: number[]) => void }
  const fill = ctx.fillRect
  ctx.fillRect = (...a: number[]) => {
    fills.n++
    fill(...a)
  }
  clock = 50_000
  const e = new DitherEngine(canvasOf.canvas, opts)
  made.push(e)
  e.setSize(180, 60, 2)
  // A fade-in, as DitherLayer mounts every field.
  e.setSources([{ id: 't', kind: 'edge', side: 'top', depth: 30, strength: 0.4 }, { ...drift, strength: 0.9 }])
  for (let f = 0; f < 90; f++) {
    const next = frames.shift()
    if (!next) break
    clock = 50_000 + 16.667 * (firstAtMount ? f : f + 1)
    next(clock)
  }
  return { px: canvasOf.pixels(), fills: fills.n, w: 360, h: 120 }
}

for (const firstAtMount of [true, false]) {
  test(`a ring trace paints its ring exactly and skips the rest — first frame ${firstAtMount ? 'at' : 'after'} the mount`, () => {
    pinClock(() => {
      frames = []
      const full = ringRun({ pitch: 4, dot: 1.6, alphaFloor: 0.1, maxAlpha: 0.6 }, firstAtMount)
      for (const e of made.splice(0)) e.destroy()
      frames = []
      const ring = ringRun({ pitch: 4, dot: 1.6, alphaFloor: 0.1, maxAlpha: 0.6, edgeBand: 2 }, firstAtMount)
      const r = 4 // the 2 px ring at DPR 2
      let ringDiff = 0
      let inside = 0
      for (let y = 0; y < full.h; y++) {
        for (let x = 0; x < full.w; x++) {
          const i = y * full.w + x
          const inRing = x < r || y < r || x >= full.w - r || y >= full.h - r
          if (inRing && full.px[i] !== ring.px[i]) ringDiff++
          // Past the ring and its slack, nothing is drawn.
          if (x >= 4 * r && y >= 4 * r && x < full.w - 4 * r && y < full.h - 4 * r && ring.px[i]! > 0) inside++
        }
      }
      assert.equal(ringDiff, 0, 'the ring shows the same pixels')
      assert.equal(inside, 0, 'nothing past the band')
      assert.ok(ring.fills < full.fills / 3, `${ring.fills} fills against ${full.fills}`)
    })
  })
}

test('a mask set during a fade-in skips the cells it hides from then on, and paints what a rebuild paints', () => {
  pinClock(() => {
    frames = []
    const mask = [{ x: 10.3, y: 8.6, w: 50.2, h: 12.5 }]
    const sources: DitherSource[] = [{ id: 'v', kind: 'uniform', strength: 0.3 }, { ...drift, strength: 0.2 }]
    const live = rasterCanvas(30.25, 20.5, 'round')
    clock = 50_000
    const e = new DitherEngine(live.canvas, { alphaFloor: 0.06, maxAlpha: 0.4 })
    made.push(e)
    e.setSize(180, 60, 2)
    e.setSources(sources)
    const painted = trackPaint(e)
    for (let f = 0; f < 60; f++) {
      if (f === 5) e.setMask(mask)
      const next = frames.shift()
      if (!next) break
      clock = 50_000 + f * 16.667
      next(clock)
    }
    const fresh = rasterCanvas(30.25, 20.5, 'round')
    const r = new DitherEngine(fresh.canvas, { alphaFloor: 0.06, maxAlpha: 0.4 })
    made.push(r)
    r.setSize(180, 60, 2)
    r.setMask(mask)
    r.setSources(sources, { immediate: true })
    ;(r as unknown as Painter).paint(painted.t, true)
    const a = live.pixels()
    const b = fresh.pixels()
    let differ = 0
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) differ++
    assert.equal(differ, 0)
  })
})

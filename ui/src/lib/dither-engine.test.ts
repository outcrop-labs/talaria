import assert from 'node:assert/strict'
import { afterEach, beforeEach, test } from 'vitest'
import { DitherEngine, type DitherEngineOptions, type DitherSource } from './dither-engine'

// The engine's frame loop and its incremental paint, against a recording 2D
// context and a hand-driven requestAnimationFrame. dither.test.ts covers the
// field math; this covers the two things the paint path promises (N11):
//
//   - a field with nothing travelling is painted and then PARKS — no frame is
//     scheduled while nothing on it changes (a speed-0 wave included);
//   - an incremental rewrite erases a dot by the whole device pixels its fill
//     touched, so it leaves the pixels a full rebuild would. A clear over the
//     dot's own fractional rect antialiases and leaves part of each edge
//     pixel, and every rewrite composited over that: live fields crept to
//     four to seven times their alpha (measured in Chromium).

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
  for (const k of ['document', 'window', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame']) {
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

afterEach(() => {
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
  e.setSize(240, 120, at.dpr)
  e.setSources(sources, { immediate: true })
  return { e, calls }
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

test('an incremental rewrite clears whole device pixels, and never reaches a neighbour', () => {
  for (const dpr of [1, 1.25, 1.5, 2]) {
    for (const [pitch, dot] of [[2, 1], [3, 1.2], [4, 1.4], [4, 1.6]] as const) {
      // A fractional page position: most real fields have one.
      frames = []
      const { e, calls } = engine([edge, { id: 'w', kind: 'wave', axis: 'x', wavelength: 90, speed: 30, strength: 0.4 }], { pitch, dot, shimmer: 0.15 }, { left: 100.5, top: 80.25, dpr })
      drive(performance.now() + 1000, 80, 12)
      e.destroy()
      // The first call is the full-canvas clear a build starts with; every
      // later clear erases one dot.
      const fills = calls.filter((c) => c.op === 'fillRect')
      const clears = calls.filter((c) => c.op === 'clearRect').filter((c) => !(c.x === 0 && c.y === 0 && c.w === 240 && c.h === 120))
      assert.ok(clears.length > 0, `dpr ${dpr} ${pitch}/${dot}: an incremental clear ran`)
      const whole = (v: number) => Math.abs(v * dpr - Math.round(v * dpr)) < 1e-6
      for (const c of clears) {
        assert.ok(whole(c.x) && whole(c.y) && whole(c.x + c.w) && whole(c.y + c.h), `dpr ${dpr} ${pitch}/${dot}: clear ${JSON.stringify(c)} is not on whole device pixels`)
        // It covers its dot's fill, and stays inside the gap to the next dot
        // (less than one device pixel past the dot on each side, and the gap
        // is at least one).
        assert.ok(c.w * dpr < (dot + 2 / dpr) * dpr + 1e-6 && c.h * dpr < (dot + 2 / dpr) * dpr + 1e-6, `dpr ${dpr} ${pitch}/${dot}: clear ${JSON.stringify(c)} is wider than its dot`)
      }
      for (const f of fills.slice(-20)) {
        const box = clears.find((c) => c.x <= f.x + 1e-9 && c.y <= f.y + 1e-9 && c.x + c.w >= f.x + f.w - 1e-9 && c.y + c.h >= f.y + f.h - 1e-9)
        if (box) assert.ok(box.x > f.x - 1 / dpr - 1e-9 && box.x + box.w < f.x + f.w + 1 / dpr + 1e-9)
      }
    }
  }
})

test('cover mode keeps the plain rect: its cells tile, and a snapped clear would reach the next', () => {
  const { calls } = engine([edge, { id: 'w', kind: 'wave', axis: 'x', wavelength: 90, speed: 30, strength: 0.4 }], { cover: true, pitch: 2 }, { left: 100.5, top: 80.25, dpr: 1 })
  drive(performance.now() + 1000, 80, 12)
  const clears = calls.filter((c) => c.op === 'clearRect').filter((c) => !(c.x === 0 && c.y === 0 && c.w === 240 && c.h === 120))
  assert.ok(clears.length > 0)
  for (const c of clears) assert.equal(c.w, 2)
})

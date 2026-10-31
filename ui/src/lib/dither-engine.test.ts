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

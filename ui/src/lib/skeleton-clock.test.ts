import assert from 'node:assert/strict'
import { afterEach, beforeEach, test } from 'vitest'
import { subscribeToClock } from './motion'
import { PITCH, attachSkeletonField, passesBayer, staticDensity, tickBucket } from './skeleton-static'

// UB6: the loading skeleton's motion is 8 Hz, and it used to cost 60 frames a
// second because the page clock woke every frame to find the 8 that change
// anything. A periodic subscriber now lets the clock sleep between its ticks.
// These run the clock and a skeleton against a virtual 60 Hz display: frames
// at each vsync, timers firing on time before the frame they precede.

let VSYNC = 1000 / 60
/** How late every timer fires, in ms: a busy main thread. */
let LATE = 0
let now = 0
let nextVsync = 0
let rafs = new Map<number, (t: number) => void>()
let timers: Array<{ id: number; at: number; cb: () => void }> = []
let seq = 0
let rafRuns = 0
const saved: Record<string, unknown> = {}

/** Advance to the next vsync: due timers first, then that frame's rAF callbacks. */
function frame(): void {
  for (;;) {
    timers.sort((a, b) => a.at - b.at || a.id - b.id)
    const t = timers[0]
    if (!t || t.at > nextVsync) break
    timers.shift()
    now = Math.max(now, t.at)
    t.cb()
  }
  now = nextVsync
  nextVsync += VSYNC
  const due = [...rafs.values()]
  rafs.clear()
  for (const cb of due) {
    rafRuns++
    cb(now)
  }
}

const GLOBALS = ['performance', 'requestAnimationFrame', 'cancelAnimationFrame', 'setTimeout', 'clearTimeout', 'window', 'document', 'getComputedStyle', 'ResizeObserver', 'IntersectionObserver', 'MutationObserver']

beforeEach(() => {
  for (const k of GLOBALS) saved[k] = (globalThis as Record<string, unknown>)[k]
  VSYNC = 1000 / 60
  LATE = 0
  now = 1000
  nextVsync = Math.ceil(now / VSYNC) * VSYNC
  rafs = new Map()
  timers = []
  rafRuns = 0
  Object.assign(globalThis, {
    performance: { now: () => now },
    requestAnimationFrame: (cb: (t: number) => void) => {
      rafs.set(++seq, cb)
      return seq
    },
    cancelAnimationFrame: (id: number) => rafs.delete(id),
    setTimeout: (cb: () => void, ms = 0) => {
      timers.push({ id: ++seq, at: now + Math.max(0, ms) + LATE, cb })
      return seq
    },
    clearTimeout: (id: number) => {
      timers = timers.filter((t) => t.id !== id)
    },
  })
})

afterEach(() => {
  for (const k of GLOBALS) (globalThis as Record<string, unknown>)[k] = saved[k]
})

test('a periodic subscriber runs on exactly the frames an every-frame loop would have picked, and the clock sleeps between', () => {
  // Reference: what the 8 Hz bucketing did on top of an every-frame clock.
  const expected: number[] = []
  const seen: number[] = []
  let last = 0
  let f = 0
  const offRef = subscribeToClock((elapsed) => {
    const b = tickBucket(elapsed)
    if (b !== last) expected.push(f)
    last = b
  })
  for (f = 0; f < 180; f++) frame()
  offRef()
  const everyFrameRuns = rafRuns

  now = 1000
  nextVsync = Math.ceil(now / VSYNC) * VSYNC
  rafRuns = 0
  let lastP = 0
  let calls = 0
  const off = subscribeToClock((elapsed) => {
    calls++
    const b = tickBucket(elapsed)
    if (b !== lastP) seen.push(f)
    lastP = b
  }, 125)
  for (f = 0; f < 180; f++) frame()
  off()

  assert.deepEqual(seen, expected, 'the same frames repaint')
  assert.equal(everyFrameRuns, 180)
  // 3 s of 8 Hz is 24 ticks. The clock wakes a frame early for each (a timer
  // can land just past the frame that would have been the first one past the
  // boundary), so a tick costs two or three frames, not seven or eight.
  assert.ok(rafRuns <= 24 * 3 + 2, `the clock ran ${rafRuns} frames for 24 ticks`)
  assert.ok(calls <= 26, `the subscriber ran ${calls} times`)
  assert.equal(rafs.size + timers.length, 0, 'nothing left pending after the last unsubscribe')
})

test('an every-frame subscriber keeps the clock on every frame, beside a periodic one', () => {
  let periodic = 0
  let each = 0
  const offA = subscribeToClock(() => periodic++, 125)
  const offB = subscribeToClock(() => each++)
  for (let f = 0; f < 60; f++) frame()
  offB()
  offA()
  assert.equal(each, 60)
  assert.ok(periodic >= 8 && periodic <= 9, `periodic ran ${periodic} times in a second`)
})

// The skeleton itself, on a recording canvas: it paints on the first frame of
// every bucket, and the clock it rides runs a fraction of the frames.
test('a loading skeleton paints once per 8 Hz bucket, the dots its bucket calls for, on a sleeping clock', () => {
  type Call = { op: string; x?: number; y?: number; style?: string }
  const calls: Call[] = []
  const ctx = {
    fillStyle: '',
    setTransform() {},
    clearRect: () => calls.push({ op: 'clear' }),
    fillRect: (x: number, y: number) => calls.push({ op: 'fill', x, y, style: ctx.fillStyle }),
  }
  class Obs {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, {
    window: { devicePixelRatio: 1, scrollX: 0, scrollY: 0, matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) },
    document: { documentElement: {}, createElement: () => ({ getContext: () => ({ fillStyle: '#e7e2db' }) }) },
    getComputedStyle: () => ({ borderTopLeftRadius: '6px', getPropertyValue: () => '#e7e2db' }),
    ResizeObserver: Obs,
    IntersectionObserver: Obs,
    MutationObserver: Obs,
  })
  const host = { getBoundingClientRect: () => ({ left: 100.4, top: 50.6, width: 64, height: 20 }) } as unknown as HTMLElement
  const canvas = { width: 0, height: 0, getContext: () => ctx } as unknown as HTMLCanvasElement
  const detach = attachSkeletonField(host, canvas)
  // The clock's origin is its first frame.
  const at0 = nextVsync
  const paintsAt: number[] = []
  const at: number[] = []
  for (let f = 0; f < 120; f++) {
    const before = calls.filter((c) => c.op === 'clear').length
    frame()
    at.push(now - at0)
    if (calls.filter((c) => c.op === 'clear').length > before) paintsAt.push(f)
  }
  detach()
  // 120 frames reach 1,983 ms on the clock: a paint on each of the 15 bucket
  // boundaries in that (125 ms is 7.5 frames, so they alternate 7 and 8 apart),
  // each on the first frame past its boundary.
  assert.equal(paintsAt.length, 15, `painted on frames ${paintsAt.join(',')}`)
  for (const f of paintsAt) assert.equal(tickBucket(at[f]!), tickBucket(at[f - 1]!) + 1, `frame ${f}`)
  assert.ok(rafRuns <= 15 * 3 + 2, `the clock ran ${rafRuns} frames for 15 paints`)

  // The last paint's dots are the ones the field math lights for its bucket.
  const lastClear = calls.map((c) => c.op).lastIndexOf('clear')
  const dots = calls.slice(lastClear + 1)
  const bucket = tickBucket(at[paintsAt.at(-1)!]!)
  const fx = 100.4 % PITCH
  const fy = 50.6 % PITCH
  let lit = 0
  for (let cy = 0; cy < Math.ceil((20 + fy) / PITCH); cy++) {
    for (let cx = 0; cx < Math.ceil((64 + fx) / PITCH); cx++) {
      const gx = Math.round((100.4 - fx) / PITCH) + cx
      const gy = Math.round((50.6 - fy) / PITCH) + cy
      const x = cx * PITCH - fx + PITCH / 2
      const y = cy * PITCH - fy + PITCH / 2
      // Inside the 6px-radius box (the corners cut a few cells).
      const qx = Math.max(6 - x, x - (64 - 6))
      const qy = Math.max(6 - y, y - (20 - 6))
      if (Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - 6 >= 0) continue
      if (passesBayer(staticDensity(gx, gy, bucket), gx, gy)) lit++
    }
  }
  assert.equal(dots.length, lit)
  assert.ok(dots.every((d) => d.op === 'fill' && /^rgba\(231,226,219,0\.\d+\)$/.test(d.style!)))
})

// The same frames at other refresh rates, and with timers firing late, up to
// the clock's one-frame lead (a later timer lands a frame late, as a stall
// that long delays an every-frame loop too).
for (const [hz, late] of [[30, 0], [120, 0], [144, 0], [60, 5], [60, 12], [120, 6]] as const) {
  test(`a periodic subscriber picks the every-frame loop's frames at ${hz} Hz, timers ${late} ms late`, () => {
    VSYNC = 1000 / hz
    LATE = late
    const run = (every: number) => {
      now = 1000
      nextVsync = Math.ceil(now / VSYNC) * VSYNC
      rafs = new Map()
      timers = []
      rafRuns = 0
      const seen: number[] = []
      let last = 0
      let f = 0
      const off = subscribeToClock((elapsed) => {
        const b = tickBucket(elapsed)
        if (b !== last) seen.push(f)
        last = b
      }, every)
      for (f = 0; f < hz * 3; f++) frame()
      off()
      return { seen, frames: rafRuns }
    }
    const reference = run(0)
    const periodic = run(125)
    assert.deepEqual(periodic.seen, reference.seen)
    assert.ok(periodic.frames < reference.frames / 2, `${periodic.frames} of ${reference.frames} frames`)
  })
}

test('the last subscriber unsubscribing inside its own callback leaves nothing pending', () => {
  let n = 0
  let off: () => void = () => {}
  off = subscribeToClock(() => {
    n++
    if (n === 3) off()
  })
  for (let f = 0; f < 10; f++) frame()
  assert.equal(n, 3)
  assert.equal(rafs.size + timers.length, 0, 'no orphaned frame loop')
})

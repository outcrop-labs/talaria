import assert from 'node:assert/strict'
import { test } from 'vitest'
import { ANIMATIONS, AMBIENT, IMMEDIATE, STANDARD } from './animations'
import { GRID_ANIMATIONS } from './grid-animations'
import { rasterise, cellsKey } from './braille'
import { clamp01 } from './field'
import { WAITING_STATES } from './registry'

// The rules the set was built to satisfy, as assertions rather than as a
// paragraph on a gallery page nobody runs. Each of these caught a real bug in
// the playground — six braille fields emptied on the first pass, Breathe and
// Checkerboard were both two-frame strobes, and Helix silently looped at double
// speed — so they are regression tests for work already done, not speculation.

/** 64 samples across one loop. Fine enough to catch a blank; cheap enough to run. */
const SAMPLES = 64
const phases = Array.from({ length: SAMPLES }, (_, i) => i / SAMPLES)

const litDots = (mask: number): number => {
  let n = 0
  for (let m = mask; m; m >>= 1) n += m & 1
  return n
}

const brailleFrame = (a: (typeof ANIMATIONS)[number], p: number): string =>
  cellsKey(rasterise(a.cols, p, a.field, a.cellAlpha))

const gridFrame = (a: (typeof GRID_ANIMATIONS)[number], p: number): number[] => {
  const out: number[] = []
  for (let y = 0; y < a.n; y++) for (let x = 0; x < a.n; x++) out.push(a.field(x, y, p, a.n, a.n))
  return out
}

test('the catalogue is 21 braille + 9 grid = 30', () => {
  assert.equal(ANIMATIONS.length, 21)
  assert.equal(GRID_ANIMATIONS.length, 9)
  assert.equal(WAITING_STATES.length, 30)
})

test('no braille state is ever fully blank', () => {
  // An indicator that empties, even for two frames, reads as FINISHED — which
  // is the one thing a waiting mark must never say.
  for (const a of ANIMATIONS) {
    for (const p of phases) {
      const cells = rasterise(a.cols, p, a.field, a.cellAlpha)
      const dots = cells.reduce((n, c) => n + litDots(c.ch.charCodeAt(0) - 0x2800), 0)
      assert.ok(dots > 0, `${a.id} is blank at phase ${p.toFixed(3)}`)
    }
  }
})

test('no grid state ever goes dark', () => {
  for (const a of GRID_ANIMATIONS) {
    for (const p of phases) {
      const sum = gridFrame(a, p).reduce((s, v) => s + clamp01(v), 0)
      assert.ok(sum > 0.05, `${a.id} is dark at phase ${p.toFixed(3)} (sum ${sum.toFixed(3)})`)
    }
  }
})

test('every state loops seamlessly — phase 1 is phase 0', () => {
  // Each field is a function of loop phase, so this should hold by
  // construction. It stops holding the moment someone writes a field in terms
  // of elapsed time, and a seam is the first thing the eye catches in
  // something that runs for thirty seconds.
  for (const a of ANIMATIONS) {
    assert.equal(brailleFrame(a, 1), brailleFrame(a, 0), `${a.id} has a seam`)
  }
  for (const a of GRID_ANIMATIONS) {
    const at0 = gridFrame(a, 0)
    const at1 = gridFrame(a, 1)
    for (let i = 0; i < at0.length; i++) {
      assert.ok(Math.abs(at0[i]! - at1[i]!) < 1e-9, `${a.id} has a seam at dot ${i}`)
    }
  }
})

test('nothing is a two-frame strobe', () => {
  // A two-state flip is not an animation, it is a blink — and Mercury already
  // ships that as gd-pulse. Anything here that degrades to two frames is
  // duplicating the fallback while costing a rAF subscription.
  for (const a of ANIMATIONS) {
    const distinct = new Set(phases.map((p) => brailleFrame(a, p)))
    assert.ok(distinct.size > 2, `${a.id} has only ${distinct.size} distinct frames`)
  }
  for (const a of GRID_ANIMATIONS) {
    const distinct = new Set(phases.map((p) => gridFrame(a, p).map((v) => v.toFixed(3)).join()))
    assert.ok(distinct.size > 2, `${a.id} has only ${distinct.size} distinct frames`)
  }
})

test('every state uses a full period, not a half one', () => {
  // Antiphase strands and rotating lines both silently repeat every HALF turn
  // — theta and theta+pi are the same line — so the loop runs at double the
  // declared speed. Helix and Sweep Hand are both built specifically to avoid
  // it, and this is what proves they still do.
  for (const a of ANIMATIONS) {
    const half = phases.every((p) => brailleFrame(a, p) === brailleFrame(a, (p + 0.5) % 1))
    assert.ok(!half, `${a.id} repeats every half period`)
  }
  for (const a of GRID_ANIMATIONS) {
    const half = phases.every((p) => {
      const x = gridFrame(a, p)
      const y = gridFrame(a, (p + 0.5) % 1)
      return x.every((v, i) => Math.abs(v - y[i]!) < 1e-6)
    })
    assert.ok(!half, `${a.id} repeats every half period`)
  }
})

test('grid intensities stay inside [0, 1]', () => {
  // The renderer writes these straight to style.opacity, where an out-of-range
  // value is clamped silently by the browser — so a field drifting past 1
  // flattens into a plateau that looks like a bug in the animation.
  for (const a of GRID_ANIMATIONS) {
    for (const p of phases) {
      for (const [i, v] of gridFrame(a, p).entries()) {
        assert.ok(v >= -1e-9 && v <= 1 + 1e-9, `${a.id} dot ${i} = ${v} at phase ${p.toFixed(3)}`)
      }
    }
  }
})

test('every period sits on the timing ladder', () => {
  // Mercury's rungs (spec §9). A few states need a longer arc to read at all —
  // a heartbeat at 1.2s is tachycardia — so whole MULTIPLES are allowed, which
  // keeps them beating against the rest rather than drifting through it.
  const rungs = [IMMEDIATE, STANDARD, AMBIENT]
  for (const a of [...ANIMATIONS, ...GRID_ANIMATIONS]) {
    const ok = rungs.some((r) => a.period % r === 0 && a.period / r >= 1)
    assert.ok(ok, `${a.id} has period ${a.period}ms, off the 750/1200/2100 ladder`)
  }
})

test('braille cell widths match what the registry advertises', () => {
  // The registry derives slot eligibility from `cols`, so a field that renders
  // wider than it declares would be dealt into a button it overflows.
  for (const a of ANIMATIONS) {
    assert.equal(rasterise(a.cols, 0, a.field, a.cellAlpha).length, a.cols, `${a.id} width`)
  }
})

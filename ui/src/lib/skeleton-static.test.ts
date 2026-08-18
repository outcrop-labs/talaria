import assert from 'node:assert/strict'
import { test } from 'vitest'
import { PITCH, insideRounded, maskRadius, passesBayer, solidity, staticDensity, tickBucket } from './skeleton-static'

// The field math, which is the half of the skeleton engine that carries the
// intent. Everything else needs a canvas, a DPR and a rAF loop.

test('static swings around its mean and never leaves the dither range', () => {
  let min = 1
  let max = 0
  let sum = 0
  let n = 0
  for (let bucket = 0; bucket < 40; bucket++) {
    for (let cy = 0; cy < 20; cy++) {
      for (let cx = 0; cx < 20; cx++) {
        const d = staticDensity(cx, cy, bucket)
        min = Math.min(min, d)
        max = Math.max(max, d)
        sum += d
        n++
      }
    }
  }
  // Mean 0.48, swing 0.55 → the band is [0.205, 0.755]. Both ends matter:
  // below ~0.2 whole neighbourhoods drop out and the block reads as a hole,
  // above ~0.76 the dots close up and it reads as the solid block this
  // replaced.
  assert.ok(min > 0.2, `floor ${min}`)
  assert.ok(max < 0.76, `ceiling ${max}`)
  assert.ok(Math.abs(sum / n - 0.48) < 0.01, `mean ${sum / n}`)
})

test('the field is deterministic per cell and per tick — it flickers, it does not drift', () => {
  assert.equal(staticDensity(3, 7, 12), staticDensity(3, 7, 12))
  assert.notEqual(staticDensity(3, 7, 12), staticDensity(3, 7, 13))
  // Neighbours are independent: noise, not a gradient.
  assert.notEqual(staticDensity(3, 7, 12), staticDensity(4, 7, 12))
})

test('a frozen bucket is a still field — what reduced motion renders', () => {
  const first = Array.from({ length: 50 }, (_, i) => staticDensity(i, i, 0))
  const again = Array.from({ length: 50 }, (_, i) => staticDensity(i, i, 0))
  assert.deepEqual(first, again)
})

test('the mask is the element box, with the corners its radius cuts', () => {
  const w = 80
  const h = 20
  assert.ok(insideRounded(40, 10, w, h, 6)) // centre
  assert.ok(!insideRounded(-1, 10, w, h, 6)) // outside the left edge
  assert.ok(!insideRounded(40, h + 1, w, h, 6)) // below the bottom
  assert.ok(!insideRounded(0.5, 0.5, w, h, 6)) // clipped corner
  assert.ok(insideRounded(0.5, 0.5, w, h, 0)) // same point, square block
})

test('an oversized radius is a capsule, not a disappearing block', () => {
  // `rounded-full` computes to a value parseFloat cannot read, so the mask
  // radius is deliberately huge. Clamped to the half-size it must still leave
  // a full-height stripe down the middle rather than mask everything away.
  const r = maskRadius('calc(infinity * 1px)', 80, 20)
  assert.ok(r >= 80)
  assert.ok(insideRounded(40, 1, 80, 20, r)) // top edge, mid-span
  assert.ok(insideRounded(10, 10, 80, 20, r)) // on the axis, near the cap
  assert.ok(!insideRounded(0.5, 0.5, 80, 20, r)) // the cap's corner is gone
  // A circle: the box's own corner is outside, its centre is in.
  assert.ok(insideRounded(12, 12, 24, 24, maskRadius('9999px', 24, 24)))
  assert.ok(!insideRounded(1, 1, 24, 24, maskRadius('9999px', 24, 24)))
})

test('a readable radius is used as written', () => {
  assert.equal(maskRadius('6px', 80, 20), 6)
  assert.equal(maskRadius('0px', 80, 20), 0)
})

test('the Bayer threshold turns density into a dot count, not an on/off', () => {
  const share = (density: number) => {
    let on = 0
    for (let cy = 0; cy < 8; cy++) for (let cx = 0; cx < 8; cx++) if (passesBayer(density, cx, cy)) on++
    return on / 64
  }
  assert.equal(share(0), 0)
  assert.equal(share(1), 1)
  assert.ok(Math.abs(share(0.48) - 0.48) < 0.02)
  // Monotonic across the band the noise actually occupies.
  assert.ok(share(0.25) < share(0.5))
  assert.ok(share(0.5) < share(0.75))
})

test('the clock is bucketed to 8Hz — eight re-rolls a second, and no more', () => {
  const seen = new Set<number>()
  // A frame every ~16.7ms for one second: 60 ticks in, 8 buckets out.
  for (let f = 0; f < 60; f++) seen.add(tickBucket((f * 1000) / 60))
  assert.equal(seen.size, 8)

  // Stable inside a bucket, advances at its edge — the skip is what makes the
  // field flicker instead of boil.
  assert.equal(tickBucket(0), tickBucket(124))
  assert.equal(tickBucket(125), tickBucket(249))
  assert.equal(tickBucket(125) - tickBucket(124), 1)
  // Monotonic, so a bucket is never revisited while the page clock runs.
  assert.ok(tickBucket(10_000) > tickBucket(9_999))
})

test('a field too small to hold a pattern goes solid instead of vanishing', () => {
  // Stated in CSS px and converted, so the thresholds keep meaning the same
  // physical thing if the pitch ever moves again — which it has once already.
  const cells = (px: number) => (px / PITCH) ** 2
  const densityAt = (px: number, mean = 0.48) => mean + (1 - mean) * solidity(cells(px))

  // A 6px status dot: at the plain mean it lights so few cells that it
  // regularly lights NONE, and at 8Hz that is a dot blinking out of existence.
  assert.ok(densityAt(6) > 0.9, `6px -> ${densityAt(6)}`)
  assert.ok(densityAt(12) > 0.7, `12px -> ${densityAt(12)}`)

  // Big enough to carry texture: untouched, so a card still reads as static.
  assert.equal(solidity(cells(20)), 0)
  assert.equal(densityAt(200), 0.48)

  // Monotonic and eased, never a step: a 16px avatar and a 20px one sit in the
  // same list and must not look like two different materials.
  let prev = 1.1
  for (let px = 0; px <= 20; px += 2) {
    const v = solidity(cells(px))
    assert.ok(v <= prev, `not monotonic at ${px}px`)
    prev = v
  }
  assert.equal(solidity(0), 1)
})

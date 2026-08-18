import assert from 'node:assert/strict'
import { test } from 'vitest'
import { evalSource, type DitherSource } from './dither'

// The field math, which is the half of the engine that carries intent and the
// half that degrades silently — a wrong exponent still paints a field, just
// not the right one. Everything else needs a canvas.
const at = (s: DitherSource, x: number, y: number, w = 200, h = 100) => evalSource(s, x, y, 0, w, h)

const rect = (over: Partial<Extract<DitherSource, { kind: 'rect' }>> = {}): DitherSource => ({
  id: 'r', kind: 'rect', x: 50, y: 30, w: 100, h: 40, spread: 20, strength: 1, ...over,
})

test('a uniform source is its strength everywhere', () => {
  const s: DitherSource = { id: 'u', kind: 'uniform', strength: 0.4 }
  assert.equal(at(s, 0, 0), 0.4)
  assert.equal(at(s, 199, 99), 0.4)
})

test('an edge is densest at its edge and gone past its depth', () => {
  const s: DitherSource = { id: 'e', kind: 'edge', side: 'top', depth: 40, strength: 1 }
  assert.equal(at(s, 10, 0), 1)
  assert.equal(at(s, 10, 40), 0)
  // Shaped, not linear: at the halfway point a linear ramp would read 0.5.
  // The curve is deliberately denser at the edge with a long quiet tail.
  assert.ok(at(s, 10, 20) < 0.5)
})

test('each edge side measures from its own side', () => {
  const depth = 40
  const mk = (side: 'top' | 'bottom' | 'left' | 'right'): DitherSource =>
    ({ id: side, kind: 'edge', side, depth, strength: 1 })
  assert.equal(at(mk('bottom'), 10, 100), 1)
  assert.equal(at(mk('left'), 0, 10), 1)
  assert.equal(at(mk('right'), 200, 10), 1)
})

test('a rect is solid inside by default and fades to nothing at its spread', () => {
  const s = rect()
  assert.equal(at(s, 100, 50), 1) // dead centre
  assert.equal(at(s, 150 + 20, 50), 0) // exactly at the spread
  assert.ok(at(s, 155, 50) > 0 && at(s, 155, 50) < 1) // inside the falloff
})

test('inner 0 with rim 0 keeps the interior CLEAN — the label has to be readable', () => {
  // The button bloom depends on this: dots behind the text made the
  // transparent variants unreadable, so the bloom lives strictly outside.
  const s = rect({ inner: 0, rim: 0 })
  assert.equal(at(s, 100, 50), 0)
  assert.equal(at(s, 60, 40), 0)
  // ...while the outside halo is untouched.
  assert.ok(at(s, 155, 50) > 0)
})

test('a rim brightens the boundary while the middle stays calm', () => {
  const s = rect({ inner: 0.2, rim: 10 })
  const middle = at(s, 100, 50)
  const nearEdge = at(s, 100, 32) // 2px inside the top boundary
  assert.ok(Math.abs(middle - 0.2) < 1e-9, `middle should settle to inner, got ${middle}`)
  assert.ok(nearEdge > middle, 'the rim must read brighter than the interior')
})

test('a higher falloff decays faster — what makes a wide nav row read concentric', () => {
  const soft = at(rect({ falloff: 2 }), 155, 50)
  const steep = at(rect({ falloff: 3 }), 155, 50)
  assert.ok(steep < soft, `falloff 3 (${steep}) must decay faster than 2 (${soft})`)
})

test('radius rounds the corners, so the halo wraps the curve', () => {
  // Just outside the top-left corner. With a radius the corner is further
  // away, so the density there is lower than the sharp-cornered field.
  const sharp = at(rect({ radius: 0 }), 52, 32)
  const round = at(rect({ radius: 12 }), 52, 32)
  assert.ok(round < sharp, `rounded corner (${round}) should be dimmer than sharp (${sharp})`)
  // An edge midpoint is unaffected by the corner radius.
  assert.equal(at(rect({ radius: 0 }), 100, 20), at(rect({ radius: 12 }), 100, 20))
})

test('a ramp holds its levels outside its span and interpolates within', () => {
  const s: DitherSource = {
    id: 'p', kind: 'ramp', axis: 'x', from: 40, to: 80, fromLevel: 1, toLevel: 0, strength: 1,
  }
  assert.equal(at(s, 0, 0), 1)
  assert.equal(at(s, 40, 0), 1)
  assert.equal(at(s, 60, 0), 0.5)
  assert.equal(at(s, 80, 0), 0)
  assert.equal(at(s, 200, 0), 0)
})

test('a zero-width ramp does not divide by zero', () => {
  const s: DitherSource = {
    id: 'p', kind: 'ramp', axis: 'x', from: 50, to: 50, fromLevel: 0, toLevel: 1, strength: 1,
  }
  assert.equal(at(s, 10, 0), 1)
})

test('a halo falls off radially and stops at its radius', () => {
  const s: DitherSource = { id: 'h', kind: 'halo', x: 100, y: 50, radius: 30, strength: 1 }
  assert.equal(at(s, 100, 50), 1)
  assert.equal(at(s, 130, 50), 0)
  assert.ok(at(s, 115, 50) > 0 && at(s, 115, 50) < 1)
})

test('a wave travels with time and never goes negative', () => {
  const s: DitherSource = {
    id: 'w', kind: 'wave', axis: 'x', wavelength: 100, speed: 50, strength: 1,
  }
  // Cubed sine is signed; a negative crest would subtract density from other
  // sources rather than adding none.
  for (let x = 0; x <= 200; x += 7) {
    for (const t of [0, 0.5, 1, 2.25]) {
      const v = evalSource(s, x, 0, t, 200, 100)
      assert.ok(v >= 0, `wave went negative at x=${x} t=${t}: ${v}`)
    }
  }
  assert.notEqual(evalSource(s, 50, 0, 0, 200, 100), evalSource(s, 50, 0, 0.7, 200, 100))
})

test('strength scales every kind, so a source can be tweened to nothing', () => {
  // The engine removes a source by tweening strength to 0; if any kind ignored
  // strength, that source would never leave.
  const kinds: DitherSource[] = [
    { id: 'a', kind: 'uniform', strength: 0 },
    { id: 'b', kind: 'edge', side: 'top', depth: 40, strength: 0 },
    { id: 'c', kind: 'rect', x: 0, y: 0, w: 50, h: 50, spread: 10, strength: 0 },
    { id: 'd', kind: 'halo', x: 10, y: 10, radius: 20, strength: 0 },
    { id: 'e', kind: 'ramp', axis: 'x', from: 0, to: 10, fromLevel: 1, toLevel: 1, strength: 0 },
    { id: 'f', kind: 'wave', axis: 'x', wavelength: 50, speed: 10, strength: 0 },
  ]
  for (const s of kinds) assert.equal(at(s, 5, 5), 0, `${s.kind} ignored strength 0`)
})

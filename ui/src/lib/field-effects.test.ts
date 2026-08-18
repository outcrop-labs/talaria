import assert from 'node:assert/strict'
import { test } from 'vitest'
import {
  edgeEffect, effectDefs, effectIndex, haloEffect, isAnimated,
  rampEffect, rectEffect, uniformEffect, waveEffect,
} from './field-effects'

// The pack functions are the contract between TypeScript and GLSL: the shader
// reads slot `a.x` and the pack decides what goes there. They are the one part
// of an effect that is pure, and the part that silently breaks — a shader
// reading the wrong slot still renders, just not the right thing.

test('every effect has a distinct index, and the order is stable', () => {
  const kinds = effectDefs().map((d) => d.kind)
  assert.equal(new Set(kinds).size, kinds.length, 'duplicate kind registered')
  // The index IS the shader's branch number, so a reordering silently
  // repoints every packed source at the wrong effect.
  for (const k of kinds) assert.ok(effectIndex(k) >= 0, `${k} has no index`)
})

test('rect packs spread, radius, inner and rim in the slots the shader reads', () => {
  const { a, b } = rectEffect.pack({ spread: 12, radius: 6, inner: 0, rim: 0, falloff: 3 })
  assert.deepEqual(a, [12, 6, 0, 0])
  assert.deepEqual(b, [3, 0, 0, 0])
})

test('rect defaults keep a bare halo solid inside, which is the engine behaviour', () => {
  // `inner` defaulting to 1 means a rect with no interior settings fills
  // solid — callers that want a clean label pass `inner: 0` explicitly.
  const { a, b } = rectEffect.pack({ spread: 10 })
  assert.deepEqual(a, [10, 0, 1, 10])
  assert.deepEqual(b, [2, 0, 0, 0])
})

test('edge encodes its side as the number the shader branches on', () => {
  // The shader compares a.x against 0.5/1.5/2.5 in this order; a different
  // mapping here would put a vignette on the wrong side of the box.
  assert.equal(edgeEffect.pack({ side: 'top', depth: 60 }).a[0], 0)
  assert.equal(edgeEffect.pack({ side: 'bottom', depth: 60 }).a[0], 1)
  assert.equal(edgeEffect.pack({ side: 'left', depth: 60 }).a[0], 2)
  assert.equal(edgeEffect.pack({ side: 'right', depth: 60 }).a[0], 3)
  assert.equal(edgeEffect.pack({ side: 'top', depth: 60 }).a[1], 60)
})

test('axis effects agree on 0 = x, 1 = y', () => {
  assert.equal(rampEffect.pack({ axis: 'x', from: 0, to: 1, fromLevel: 1, toLevel: 0 }).a[0], 0)
  assert.equal(rampEffect.pack({ axis: 'y', from: 0, to: 1, fromLevel: 1, toLevel: 0 }).a[0], 1)
  assert.equal(waveEffect.pack({ axis: 'x', wavelength: 100, speed: 9 }).a[0], 0)
  assert.equal(waveEffect.pack({ axis: 'y', wavelength: 100, speed: 9 }).a[0], 1)
})

test('ramp keeps its levels in the second slot, since the first is full', () => {
  const { a, b } = rampEffect.pack({ axis: 'x', from: 40, to: 80, fromLevel: 1, toLevel: 0 })
  assert.deepEqual(a, [0, 40, 80, 0])
  assert.deepEqual(b, [1, 0, 0, 0])
})

test('halo and uniform pack without a second slot', () => {
  assert.deepEqual(haloEffect.pack({ x: 10, y: 20, radius: 30 }).a, [10, 20, 30, 0])
  assert.equal(haloEffect.pack({ x: 10, y: 20, radius: 30 }).b, undefined)
  assert.deepEqual(uniformEffect.pack({}).a, [0, 0, 0, 0])
})

test('only the wave asks for another frame', () => {
  // Everything else is static once drawn; an idle surface must stop scheduling
  // or every field on the page costs a repaint forever.
  assert.equal(isAnimated('wave'), true)
  for (const k of ['rect', 'edge', 'uniform', 'halo', 'ramp']) {
    assert.equal(isAnimated(k), false, `${k} should not animate`)
  }
})

import assert from 'node:assert/strict'
import { test } from 'vitest'
import { latticeOrigin } from './dither'

/*
 * WHAT IS TESTED HERE, AND WHAT IS NOT.
 *
 * This file used to test `evalSource` — the CPU evaluator for every source
 * kind: the rounded-rect signed distance, the clean interior a label sits on,
 * the falloff exponent that makes a wide nav row read concentric. That code is
 * gone. Those semantics live in GLSL now (`lib/field-effects.ts`), and are
 * exercised by `field-effects.test.ts`, which tests the pack contracts — the
 * boundary where a mistake is silent — rather than the arithmetic.
 *
 * Keeping `evalSource` alive purely so it could be unit tested was considered
 * and rejected: a second implementation of the field that nothing renders is
 * one that can drift from the shader without any test noticing, and two
 * renderers disagreeing is precisely the problem the shader rewrite existed to
 * end. The coverage loss is real and is the price of having ONE field.
 *
 * The lattice is different and stays: `latticeOrigin` is shared by the shader
 * and the skeleton static, and it is what keeps their grids in phase.
 */


// ── the page-wide lattice ───────────────────────────────────────────────────
// Both canvas fields in the app key their grid to the document rather than to
// their own canvas, so a bloom and a skeleton sitting near each other are
// windows onto ONE material instead of two patterns up to a pitch out of
// phase. This is the arithmetic they share, so it is the arithmetic that has
// to agree.

test('a canvas flush with the lattice has no offset', () => {
  assert.deepEqual(latticeOrigin(0, 4), { frac: 0, cell: 0 })
  assert.deepEqual(latticeOrigin(16, 4), { frac: 0, cell: 4 })
})

test('a canvas mid-cell reports how far in it starts', () => {
  // 18px into a 4px pitch: two whole cells past cell 4, starting 2px in.
  assert.deepEqual(latticeOrigin(18, 4), { frac: 2, cell: 4 })
  assert.deepEqual(latticeOrigin(19, 4), { frac: 3, cell: 4 })
})

test('negative page coordinates stay on the same lattice', () => {
  // A bled canvas can start left of the viewport origin. A plain `%` would
  // return a negative remainder here and shift the dots off the grid.
  assert.deepEqual(latticeOrigin(-2, 4), { frac: 2, cell: -1 })
  assert.deepEqual(latticeOrigin(-4, 4), { frac: 0, cell: -1 })
  assert.deepEqual(latticeOrigin(-5, 4), { frac: 3, cell: -2 })
})

test('frac is always inside one pitch, and cell*pitch+frac reconstructs the input', () => {
  for (const px of [-9.5, -4, -0.25, 0, 1, 3.75, 12, 18.2, 101]) {
    const { frac, cell } = latticeOrigin(px, 4)
    assert.ok(frac >= 0 && frac < 4, `frac out of range for ${px}: ${frac}`)
    assert.ok(Math.abs(cell * 4 + frac - px) < 1e-9, `does not reconstruct ${px}`)
  }
})

test('two canvases a whole number of cells apart share a phase', () => {
  // The property that makes the two fields one material.
  const a = latticeOrigin(100, 4)
  const b = latticeOrigin(140, 4)
  assert.equal(a.frac, b.frac)
  assert.equal(b.cell - a.cell, 10)
})

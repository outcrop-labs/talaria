import assert from 'node:assert/strict'
import { describe, test } from 'vitest'
import { blueprintSummary, canDeleteBlueprint } from './blueprints'

// The two decisions a blueprint row makes before it renders: what it says
// under its name, and whether it offers a delete.

describe('blueprintSummary — the line under a blueprint name', () => {
  test('the first line that says something', () => {
    assert.equal(blueprintSummary('Draft the weekly brief\nThen send it'), 'Draft the weekly brief')
    assert.equal(blueprintSummary('\n\n  Draft the weekly brief'), 'Draft the weekly brief')
  })

  test('the markdown that makes it a heading, a quote or a bullet comes off', () => {
    assert.equal(blueprintSummary('# Weekly brief\nbody'), 'Weekly brief')
    assert.equal(blueprintSummary('> Quote first\nbody'), 'Quote first')
    assert.equal(blueprintSummary('- First step'), 'First step')
    assert.equal(blueprintSummary('1. First step'), 'First step')
  })

  test('emphasis is not a bullet and stays put', () => {
    assert.equal(blueprintSummary('**Goal**: ship it'), '**Goal**: ship it')
  })

  test('an empty body says nothing rather than guessing', () => {
    assert.equal(blueprintSummary(''), '')
    assert.equal(blueprintSummary('\n   \n#  \n'), '')
  })

  test('a body with no newlines cannot hand the menu a 20,000-character row', () => {
    assert.equal(blueprintSummary('x'.repeat(20_000)).length, 120)
  })
})

describe('canDeleteBlueprint — only the viewer’s own', () => {
  test('their own personal one', () => {
    assert.equal(canDeleteBlueprint({ ownerUserId: 'u1' }, 'u1'), true)
  })

  test('someone else’s, a shared one, and a viewer we do not know yet', () => {
    assert.equal(canDeleteBlueprint({ ownerUserId: 'u9' }, 'u1'), false)
    assert.equal(canDeleteBlueprint({ ownerUserId: null }, 'u1'), false)
    assert.equal(canDeleteBlueprint({ ownerUserId: 'u1' }, null), false)
    assert.equal(canDeleteBlueprint({ ownerUserId: null }, null), false)
  })
})

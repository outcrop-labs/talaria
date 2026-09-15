import assert from 'node:assert/strict'
import { describe, test } from 'vitest'
import { menuHandlesKey, shouldSendOnEnter } from './composer-keys'

// The composer's Enter contract, encoded as pure decisions the .svelte layers
// call: TALA-5 — Enter was intermittently swallowed because the suggestion
// menus claimed every key even with an empty candidate list, so the send
// keymap never saw the press. These tests pin the fall-through.

describe('menuHandlesKey — the suggestion menus decide which keys they claim', () => {
  test('an empty candidate list claims nothing — Enter must reach the send keymap', () => {
    for (const key of ['Enter', 'Tab', 'ArrowDown', 'ArrowUp', 'Escape']) {
      assert.equal(menuHandlesKey(key, []), false, `${key} must not be claimed by an empty menu`)
    }
  })

  test('a populated menu claims selection and navigation keys', () => {
    const items = ['a', 'b']
    for (const key of ['Enter', 'Tab', 'ArrowDown', 'ArrowUp']) {
      assert.equal(menuHandlesKey(key, items), true, `${key} belongs to a populated menu`)
    }
  })

  test('a populated menu never claims typing or send-adjacent keys', () => {
    const items = ['a']
    for (const key of ['a', 'Shift-Enter', 'Escape', 'Backspace']) {
      assert.equal(menuHandlesKey(key, items), false, `${key} must pass through`)
    }
  })
})

describe("shouldSendOnEnter — the send keymap's decision", () => {
  test('plain Enter sends', () => {
    assert.equal(shouldSendOnEnter({ inCodeBlock: false }), true)
  })

  test('Enter inside a code block writes code instead of sending', () => {
    assert.equal(shouldSendOnEnter({ inCodeBlock: true }), false)
  })

  test('an Enter that belongs to an active composition never sends', () => {
    assert.equal(shouldSendOnEnter({ inCodeBlock: false, compositionActive: true }), false)
    assert.equal(shouldSendOnEnter({ inCodeBlock: true, compositionActive: true }), false)
  })

  test('composition already ended — Enter sends again', () => {
    assert.equal(shouldSendOnEnter({ inCodeBlock: false, compositionActive: false }), true)
  })
})

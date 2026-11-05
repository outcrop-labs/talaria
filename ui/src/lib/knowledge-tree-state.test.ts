import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { kbTreeState, resetKbTreeState } from './knowledge-tree-state.svelte'

// These helpers are the view-memory.test.ts pattern, copied in — they are
// test plumbing, not a module worth exporting.

function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial))
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, v),
    },
  })
  return map
}

/** A storage that refuses every write — private mode. */
function installReadOnlyStorage() {
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: () => null,
      setItem: () => {
        throw new Error('QuotaExceededError')
      },
    },
  })
}

beforeEach(() => {
  vi.unstubAllGlobals()
  resetKbTreeState()
})

test('a toggled doc or section round-trips through storage', () => {
  installStorage()
  kbTreeState.toggleDoc('d-1', true)
  kbTreeState.toggleSection('s-1', true)
  // The store persists the full shape on every mutation, so a fresh
  // session (reset re-arms the hydrate latch) reads it back.
  resetKbTreeState()
  assert.equal(kbTreeState.docExpanded('d-1'), true)
  assert.equal(kbTreeState.sectionExpanded('s-1'), true)
  // And a close is remembered too, not just the open default.
  kbTreeState.toggleDoc('d-1', false)
  resetKbTreeState()
  assert.equal(kbTreeState.docExpanded('d-1'), false)
})

test('nothing stored is all defaults, not a crash', () => {
  installStorage()
  assert.equal(kbTreeState.docExpanded('d-1'), false)
  assert.equal(kbTreeState.sectionExpanded('s-1'), false)
})

test('unparseable JSON degrades to no memory', () => {
  installStorage({ 'talaria:knowledge-tree-state': '{{{' })
  assert.equal(kbTreeState.docExpanded('d-1'), false)
  assert.equal(kbTreeState.sectionExpanded('s-1'), false)
})

test('a stored shape with non-boolean values drops those entries and keeps the valid ones', () => {
  installStorage({
    'talaria:knowledge-tree-state': JSON.stringify({
      docs: { 'd-1': true, 'd-2': 'yes', 'd-3': 1 },
      spaces: { 's-1': true, 's-2': null },
    }),
  })
  assert.equal(kbTreeState.docExpanded('d-1'), true)
  assert.equal(kbTreeState.docExpanded('d-2'), false) // dropped, not crashed
  assert.equal(kbTreeState.docExpanded('d-3'), false)
  assert.equal(kbTreeState.sectionExpanded('s-1'), true)
  assert.equal(kbTreeState.sectionExpanded('s-2'), false)
})

test('a stored shape that is not an object at all is no memory', () => {
  installStorage({ 'talaria:knowledge-tree-state': JSON.stringify('nope') })
  assert.equal(kbTreeState.docExpanded('d-1'), false)
})

test('private mode still remembers for this session', () => {
  installReadOnlyStorage()
  kbTreeState.toggleDoc('d-1', true)
  assert.equal(kbTreeState.docExpanded('d-1'), true) // the reactive mirror serves the tab
})

test('openPath opens the chain and the section, and closes nothing', () => {
  installStorage()
  kbTreeState.toggleDoc('d-other', true)
  kbTreeState.openPath(['d-1', 'd-1-child'], 's-1')
  assert.equal(kbTreeState.docExpanded('d-1'), true)
  assert.equal(kbTreeState.docExpanded('d-1-child'), true)
  assert.equal(kbTreeState.sectionExpanded('s-1'), true)
  assert.equal(kbTreeState.docExpanded('d-other'), true) // never closed
  // And it persists, not just in the reactive mirror.
  resetKbTreeState()
  assert.equal(kbTreeState.docExpanded('d-1'), true)
  assert.equal(kbTreeState.docExpanded('d-other'), true)
  assert.equal(kbTreeState.sectionExpanded('s-1'), true)
})

test('a toggle matching the current state does not rewrite storage', () => {
  const map = installStorage()
  kbTreeState.toggleDoc('d-1', false) // already the default false
  assert.equal(map.has('talaria:knowledge-tree-state'), false)
})
test('resetKbTreeState clears both the memory and the reactive store', () => {
  installStorage()
  kbTreeState.toggleDoc('d-1', true)
  kbTreeState.toggleSection('s-1', true)
  resetKbTreeState()
  // reset re-arms the hydrate latch, so the next read would pull the old
  // sessionStorage right back in — a fresh (empty) storage makes the
  // "cleared" claims honest.
  const map = installStorage()
  assert.equal(kbTreeState.docExpanded('d-1'), false)
  assert.equal(kbTreeState.sectionExpanded('s-1'), false)
  kbTreeState.toggleDoc('d-2', true)
  assert.equal(map.get('talaria:knowledge-tree-state'), JSON.stringify({ docs: { 'd-2': true }, spaces: {} }))
})
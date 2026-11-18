// @vitest-environment runes
//
import { mountHarness } from './kb-tree-derive-harness.svelte'

// These helpers are the view-memory.test.ts pattern, copied in — they are
// test plumbing, not a module worth exporting.
//
// Whole-file note: this suite runs in the runes environment (Svelte's client
// runtime, no DOM) so the $effect.root/$derived harness test below observes
// real reactivity; the earlier tests are plain reads/writes, unaffected.

import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { kbTreeState, resetKbTreeState } from './knowledge-tree-state.svelte'

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

test('THE $DERIVED CRASH: a stored shape read first inside a derived/template context hydrates without state_unsafe_mutation', () => {
  // The live crash this branch's fix is for: with a stored key, hydrate()'s
  // first call wrote $state (`store.docs = saved.docs`), and on /knowledge
  // that first call was KbDocRow's `$derived(kbTreeState.docExpanded(...))`
  // and the sidebar's `{#if ... sectionExpanded(...)}` — Svelte forbids a
  // state write inside either, and loading /knowledge threw
  // state_unsafe_mutation and took the whole view down. Knowledge.svelte now
  // calls `kbTreeState.hydrate()` eagerly in its instance script, so every
  // derived/template read is a pure read. This suite has no DOM to mount a
  // component with, so the harness is the compiled shape of that row: a
  // class whose $derived reads are the crash site, evaluated inside
  // $effect.root the way a mounted component's deriveds run. See
  // kb-tree-derive-harness.svelte.ts.
  installStorage({ 'talaria:knowledge-tree-state': JSON.stringify({ docs: { 'd-1': true }, spaces: { 's-1': true } }) })
  // WITHOUT the eager hydrate, the mount below throws the live crash —
  // `state_unsafe_mutation: Updating state inside $derived(...)` — because
  // the derived's first read is the hydrate, and hydrate writes $state.
  // Knowledge.svelte's instance script now makes this exact call first, so
  // the deriveds evaluate against an already-hydrated store, as pure reads.
  kbTreeState.hydrate()
  const mounted = mountHarness() // throws state_unsafe_mutation if a derived read ever hydrates again
  // The restored shape, not the collapsed default: the deriveds follow the
  // hydrated store, and the crash they replaced never threw.
  assert.equal(mounted.row.expanded, true)
  assert.equal(mounted.row.sectionOpen, true)
  mounted.stop()
  // And that eager hydrate is idempotent: a second call (the read paths'
  // internal one) is a no-op, not a re-write.
  kbTreeState.hydrate()
  assert.equal(kbTreeState.docExpanded('d-1'), true)
})
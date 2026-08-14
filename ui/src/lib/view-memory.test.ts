import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { forgetView, hrefFor, rememberView, resetViewMemory, sectionKey } from './view-memory'

function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial))
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
      setItem: (key: string, value: string) => void map.set(key, value),
    },
    dispatchEvent: () => true,
    addEventListener: () => {},
    removeEventListener: () => {},
  })
  return map
}

beforeEach(() => {
  vi.unstubAllGlobals()
  resetViewMemory()
})

test('a section is the first path segment, so a board and its ticket share one', () => {
  assert.equal(sectionKey('/'), '/')
  assert.equal(sectionKey('/comms'), '/comms')
  assert.equal(sectionKey('/boards'), '/boards')
  assert.equal(sectionKey('/boards/b1'), '/boards')
  assert.equal(sectionKey('/boards/b1/t9'), '/boards')
})

test('an app’s two surfaces are separate sections', () => {
  // /x/<slug> is a path prefix of /x/<slug>/manage. Collapsing them would make
  // the Manage nav item navigate to the work surface — the same trap NavRail's
  // `exactFor` exists to avoid for active-state matching.
  assert.equal(sectionKey('/x/contacts'), '/x/contacts')
  assert.equal(sectionKey('/x/contacts/manage'), '/x/contacts/manage')
  assert.notEqual(sectionKey('/x/contacts'), sectionKey('/x/contacts/manage'))
})

test('the rail points at where you were, not at the bare path', () => {
  installStorage()
  assert.equal(hrefFor('/comms'), '/comms')
  rememberView('/comms?c=channel-7')
  assert.equal(hrefFor('/comms'), '/comms?c=channel-7')
  // The board you had open, in the layout you had it in.
  rememberView('/boards/b1?view=list&group=priority')
  assert.equal(hrefFor('/boards'), '/boards/b1?view=list&group=priority')
})

test('one section’s memory never answers for another', () => {
  installStorage()
  rememberView('/comms?c=channel-7')
  assert.equal(hrefFor('/boards'), '/boards')
  assert.equal(hrefFor('/'), '/')
})

test('Home remembers its tab but never a deeper path', () => {
  installStorage()
  rememberView('/?tab=activity')
  assert.equal(hrefFor('/'), '/?tab=activity')
})

test('forgetting a section returns the rail to the bare path', () => {
  installStorage()
  rememberView('/boards/b1?view=gantt')
  assert.equal(hrefFor('/boards'), '/boards/b1?view=gantt')
  // What Board.svelte does when the board turns out to be deleted: without it
  // every click on Boards walks back into the same dead end.
  forgetView('/boards/b1')
  assert.equal(hrefFor('/boards'), '/boards')
})

test('a poisoned memory cannot redirect a nav link off-section', () => {
  // sessionStorage is writable by anything on this origin, and the value ends
  // up in an href. Protocol-relative is the one that leaves the site.
  installStorage({
    'talaria:view-memory': JSON.stringify({
      '/comms': '//evil.example.com/comms',
      '/boards': 'https://evil.example.com/boards',
      '/plan': '/research?x=1',
    }),
  })
  assert.equal(hrefFor('/comms'), '/comms')
  assert.equal(hrefFor('/boards'), '/boards')
  assert.equal(hrefFor('/plan'), '/plan')
})

test('a malformed store is an empty memory, not a crash', () => {
  installStorage({ 'talaria:view-memory': 'not json at all' })
  assert.equal(hrefFor('/comms'), '/comms')
  resetViewMemory()
  installStorage({ 'talaria:view-memory': JSON.stringify(['an', 'array']) })
  assert.equal(hrefFor('/comms'), '/comms')
  resetViewMemory()
  installStorage({ 'talaria:view-memory': JSON.stringify({ '/comms': 42 }) })
  assert.equal(hrefFor('/comms'), '/comms')
})

test('storage that throws still remembers for this tab', () => {
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: () => {
        throw new Error('private mode')
      },
      setItem: () => {
        throw new Error('private mode')
      },
    },
    dispatchEvent: () => true,
    addEventListener: () => {},
    removeEventListener: () => {},
  })
  rememberView('/comms?c=channel-7')
  assert.equal(hrefFor('/comms'), '/comms?c=channel-7')
})

test('a location with no state is remembered as the bare path', () => {
  installStorage()
  rememberView('/knowledge')
  assert.equal(hrefFor('/knowledge'), '/knowledge')
})

// ── The bug this file exists to stop coming back ────────────────────────────
// You are in an agent thread, you go to a board, you click Comms, and you land
// on the first channel. The memory was not missing — it was overwritten with a
// bare `/comms` on the way out, by a URL that existed for a few milliseconds
// and that nobody navigated to.

test('a bare path never overwrites the same page’s remembered state', () => {
  installStorage()
  rememberView('/comms?a=hermes&x=conv-4')
  // What leaving Comms produces: the search is cleared a dozen lines before the
  // pathname moves, and a still-mounted view re-runs its default selection
  // against the empty URL. Either way the href is the page with its state
  // stripped, which is not a place anyone chose to be.
  rememberView('/comms')
  assert.equal(hrefFor('/comms'), '/comms?a=hermes&x=conv-4')
})

test('a real move to a different page in the section still wins', () => {
  installStorage()
  rememberView('/boards/b1?view=gantt')
  // The boards INDEX is a page you can sit on, not a stripped board.
  rememberView('/boards')
  assert.equal(hrefFor('/boards'), '/boards')
  rememberView('/boards/b2')
  assert.equal(hrefFor('/boards'), '/boards/b2')
})

test('a later selection on the same page replaces the earlier one', () => {
  installStorage()
  rememberView('/comms?c=channel-1')
  rememberView('/comms?a=hermes&x=conv-4')
  assert.equal(hrefFor('/comms'), '/comms?a=hermes&x=conv-4')
  rememberView('/comms?c=channel-9')
  assert.equal(hrefFor('/comms'), '/comms?c=channel-9')
})

test('an href is parsed as one value, so a pathname and a search cannot disagree', () => {
  installStorage()
  // The old signature took the two separately and the caller read them from two
  // reactive mirrors that update at different moments in one navigation.
  rememberView('/comms?a=hermes&x=conv-4')
  assert.equal(hrefFor('/comms'), '/comms?a=hermes&x=conv-4')
  assert.equal(hrefFor('/boards'), '/boards')
})

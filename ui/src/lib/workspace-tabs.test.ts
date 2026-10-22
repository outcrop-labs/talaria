import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import {
  closeTab,
  isClosable,
  labelTab,
  openTab,
  seedTabs,
  selectTab,
  syncTouched,
  tabsFor,
  type WorkspaceTab,
} from './workspace-tabs.svelte'

// The store is MODULE state and nothing here resets it, on purpose: every test
// works a session id of its own, which is also how the real thing behaves —
// one strip per session, never a shared one.
let n = 0
const nextSession = () => `sess-${++n}`

const STORE_KEY = 'talaria:workspace-tab:v1'

const DOC: WorkspaceTab = { id: 'doc', kind: 'doc', label: 'Brief' }

function installLocalStorage() {
  const map = new Map<string, string>()
  vi.stubGlobal('window', {
    localStorage: {
      getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
    },
  })
  return map
}

const ids = (sessionId: string) => tabsFor(sessionId).tabs.map((t) => t.id)
const stored = (map: Map<string, string>) => JSON.parse(map.get(STORE_KEY) ?? 'null') as unknown

beforeEach(() => vi.unstubAllGlobals())

test('the same document opened twice is one tab, carrying the fresher label', () => {
  const s = nextSession()
  openTab(s, DOC)
  openTab(s, { id: 'a1', kind: 'artifact', label: 'Draft note', ref: 'art_1' })
  openTab(s, { id: 'a1', kind: 'artifact', label: 'Renamed note' })
  assert.deepEqual(ids(s), ['doc', 'a1'])
  assert.equal(tabsFor(s).activeId, 'a1', 'an open focuses the tab')
  assert.equal(tabsFor(s).active?.label, 'Renamed note')
  assert.equal(tabsFor(s).tabs[1]?.ref, 'art_1', 'a label-only re-open keeps what the body renders')
})

test('a touched document opens a tab, and touching it again focuses that one', () => {
  const s = nextSession()
  openTab(s, DOC)
  syncTouched(s, { kind: 'artifact', id: 'a1', label: 'Findings' })
  assert.deepEqual(ids(s), ['doc', 'a1'])
  assert.equal(tabsFor(s).activeId, 'a1')

  // Read something else, then let the agent touch the artifact again: the pane
  // follows the agent rather than opening a second copy of the same document.
  selectTab(s, 'doc')
  assert.equal(tabsFor(s).activeId, 'doc')
  syncTouched(s, { kind: 'artifact', id: 'a1', label: 'Findings v2' })
  assert.deepEqual(ids(s), ['doc', 'a1'])
  assert.equal(tabsFor(s).activeId, 'a1')
  assert.equal(tabsFor(s).active?.label, 'Findings v2')
})

test('closing the tab you are reading lands on the neighbour', () => {
  const s = nextSession()
  openTab(s, DOC)
  openTab(s, { id: 'a1', kind: 'artifact', label: 'One' })
  openTab(s, { id: 'a2', kind: 'artifact', label: 'Two' })
  openTab(s, { id: 'a3', kind: 'artifact', label: 'Three' })

  selectTab(s, 'a2')
  closeTab(s, 'a2')
  assert.deepEqual(ids(s), ['doc', 'a1', 'a3'])
  assert.equal(tabsFor(s).activeId, 'a3', 'the tab that slid into its place')

  closeTab(s, 'a3')
  assert.equal(tabsFor(s).activeId, 'a1', 'the one before it, at the end of the strip')

  // Closing a tab you are not reading leaves the reading where it was.
  selectTab(s, 'doc')
  closeTab(s, 'a1')
  assert.deepEqual(ids(s), ['doc'])
  assert.equal(tabsFor(s).activeId, 'doc')
})

test('every tab closes — the plan document too, and the last one leaves an empty strip', () => {
  // A work session has no unclosable document any more: its pane is its
  // files. A plan's document re-opens itself when the strip empties, and that
  // is the harness's rule rather than the store's.
  const s = nextSession()
  openTab(s, DOC)
  openTab(s, { id: 'r1', kind: 'report', label: 'Run report' })
  closeTab(s, 'doc')
  assert.deepEqual(ids(s), ['r1'])
  assert.equal(tabsFor(s).activeId, 'r1')

  closeTab(s, 'r1')
  assert.deepEqual(ids(s), [])
  assert.equal(tabsFor(s).active, null)

  for (const kind of ['doc', 'artifact', 'google', 'report', 'drafts'] as const) {
    assert.equal(isClosable({ id: kind, kind, label: kind }), true, kind)
  }
})

test('seeding puts a session\'s files on the strip in order, landing on the focus it names', () => {
  const s = nextSession()
  seedTabs(
    s,
    [
      { id: 'artifact:a', kind: 'artifact', label: 'A', glyph: 'doc' },
      { id: 'artifact:b', kind: 'artifact', label: 'B', glyph: 'sheet' },
      { id: 'google:g', kind: 'google', label: 'G', glyph: 'google-doc' },
    ],
    'artifact:b',
  )
  assert.deepEqual(ids(s), ['artifact:a', 'artifact:b', 'google:g'])
  assert.equal(tabsFor(s).activeId, 'artifact:b')
  assert.equal(tabsFor(s).tabs[1]?.glyph, 'sheet')
})

test('seeding never moves somebody\'s reading, and merges the fresher fields', () => {
  const s = nextSession()
  seedTabs(s, [{ id: 'artifact:a', kind: 'artifact', label: 'A' }], 'artifact:a')
  selectTab(s, 'artifact:a')
  // A later seed (the report tab opened first, a remount) appends and renames,
  // and its focus is ignored because the session already has an active tab.
  seedTabs(
    s,
    [
      { id: 'artifact:a', kind: 'artifact', label: 'A renamed' },
      { id: 'artifact:c', kind: 'artifact', label: 'C' },
    ],
    'artifact:c',
  )
  assert.deepEqual(ids(s), ['artifact:a', 'artifact:c'])
  assert.equal(tabsFor(s).activeId, 'artifact:a')
  assert.equal(tabsFor(s).active?.label, 'A renamed')
})

test('seeding with no focus, or a focus not on the strip, lands on the first tab', () => {
  const s = nextSession()
  seedTabs(s, [{ id: 'x', kind: 'artifact', label: 'X' }, { id: 'y', kind: 'artifact', label: 'Y' }], 'nope')
  assert.equal(tabsFor(s).activeId, 'x')
})

test('the remembered tab outranks the seeded focus — and the agent\'s touch outranks both', () => {
  const map = installLocalStorage()
  const s = nextSession()
  map.set(STORE_KEY, JSON.stringify({ [s]: 'artifact:old' }))
  seedTabs(
    s,
    [
      { id: 'artifact:old', kind: 'artifact', label: 'Old' },
      { id: 'artifact:new', kind: 'artifact', label: 'Newest touched' },
    ],
    'artifact:new',
  )
  assert.equal(tabsFor(s).activeId, 'artifact:old', 'remembered > newest touched')

  syncTouched(s, { id: 'artifact:made', kind: 'artifact', label: 'Just made', glyph: 'page' })
  assert.equal(tabsFor(s).activeId, 'artifact:made', 'a file the agent just touched takes the pane')
  assert.deepEqual(stored(map), { [s]: 'artifact:made' })
})

test('no localStorage: the strip opens, focuses and closes all the same', () => {
  // No `window` at all — both the SSR pass and a browser with site data
  // blocked, where the ACCESS throws rather than only the write.
  assert.equal(typeof globalThis.window, 'undefined')
  const s = nextSession()
  openTab(s, DOC)
  syncTouched(s, { kind: 'google', id: 'g1', label: 'Sheet' })
  assert.equal(tabsFor(s).activeId, 'g1')
  selectTab(s, 'doc')
  closeTab(s, 'g1')
  assert.deepEqual(ids(s), ['doc'])
  assert.equal(tabsFor(s).activeId, 'doc')
})

test('the tab you were reading is remembered per session, under one key', () => {
  const map = installLocalStorage()
  const s1 = nextSession()
  const s2 = nextSession()
  openTab(s1, DOC)
  openTab(s1, { id: 'd1', kind: 'drafts', label: 'Drafts' })
  selectTab(s1, 'doc')
  openTab(s2, DOC)
  assert.deepEqual(stored(map), { [s1]: 'doc', [s2]: 'doc' })
  assert.equal(map.size, 1, 'one key for every session, not one key each')
})

test('a remembered tab outranks the mount-time seeding, until the viewer picks another', () => {
  const map = installLocalStorage()
  const s = nextSession()
  map.set(STORE_KEY, JSON.stringify({ [s]: 'r1' }))

  // The pane seeds the strip a tab at a time as the session's documents
  // arrive. The report was seeded in the middle, so without the memory the
  // artifact seeded last would be what you land on.
  openTab(s, DOC)
  openTab(s, { id: 'r1', kind: 'report', label: 'Run report' })
  openTab(s, { id: 'a1', kind: 'artifact', label: 'Findings' })
  assert.equal(tabsFor(s).activeId, 'r1')

  selectTab(s, 'a1')
  assert.equal(tabsFor(s).activeId, 'a1')
  assert.deepEqual(stored(map), { [s]: 'a1' })
})

test('a remembered tab the session never opened is simply not there', () => {
  const map = installLocalStorage()
  const s = nextSession()
  map.set(STORE_KEY, JSON.stringify({ [s]: 'a-deleted-artifact' }))
  openTab(s, DOC)
  assert.equal(tabsFor(s).activeId, 'doc')
})

test('a malformed stored value reads as no memory at all', () => {
  const map = installLocalStorage()
  for (const raw of ['not json', '"doc"', '[]', 'null', '42']) {
    const s = nextSession()
    map.set(STORE_KEY, raw)
    openTab(s, DOC)
    assert.equal(tabsFor(s).activeId, 'doc', raw)
  }
})

test('no session, no strip', () => {
  assert.deepEqual(tabsFor(null).tabs, [])
  assert.equal(tabsFor(null).active, null)
  assert.equal(tabsFor('never-opened').activeId, null)
})

test('labelTab renames an open tab without moving focus', () => {
  // A touched document opens at once under a placeholder and takes its real
  // title when the read lands — the rename must not steal the active tab from
  // whatever the person is reading.
  const sess = nextSession()
  openTab(sess, DOC)
  openTab(sess, { id: 'a1', kind: 'artifact', label: 'Document', ref: 'a1' })
  selectTab(sess, 'doc')
  labelTab(sess, 'a1', 'Q3 launch plan')
  assert.equal(tabsFor(sess).tabs.find((t) => t.id === 'a1')?.label, 'Q3 launch plan')
  assert.equal(tabsFor(sess).activeId, 'doc')
})

test('labelTab on an unknown tab is a no-op', () => {
  const sess = nextSession()
  openTab(sess, DOC)
  const before = JSON.stringify(tabsFor(sess))
  labelTab(sess, 'nope', 'Anything')
  assert.equal(JSON.stringify(tabsFor(sess)), before)
})

test('labelTab with the label already in place rewrites nothing', () => {
  const sess = nextSession()
  openTab(sess, { id: 'a2', kind: 'artifact', label: 'Same', ref: 'a2' })
  const tab = tabsFor(sess).tabs[0]
  labelTab(sess, 'a2', 'Same')
  // Same object, not a fresh copy: an identical label is not a change.
  assert.equal(tabsFor(sess).tabs[0], tab)
})

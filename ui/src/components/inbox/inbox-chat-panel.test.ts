import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { PANEL_COLLAPSED_KEY, readPanelCollapsed, writePanelCollapsed } from './inbox-chat-panel'

// A hand-rolled localStorage: happy-dom/jsdom may or may not be the vitest
// environment for this project, and the behaviour under test is entirely about
// what the three storage states decode to.
function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial))
  vi.stubGlobal('window', {
    localStorage: {
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
})

test('the assistant panel is closed when nothing has been stored', () => {
  installStorage()
  assert.equal(readPanelCollapsed(), true)
})

test('an explicit open survives a reload', () => {
  const map = installStorage()
  writePanelCollapsed(false)
  assert.equal(map.get(PANEL_COLLAPSED_KEY), '0')
  assert.equal(readPanelCollapsed(), false)
})

test('an explicit close reads back as closed', () => {
  installStorage({ [PANEL_COLLAPSED_KEY]: '1' })
  assert.equal(readPanelCollapsed(), true)
})

test('storage that throws falls back to closed, not open', async () => {
  vi.stubGlobal('window', {
    localStorage: {
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
  // Fresh module: the in-memory fallback is module state that an earlier test
  // in this file has already written to, and the claim here is about its
  // STARTING value.
  vi.resetModules()
  const fresh = await import('./inbox-chat-panel')
  assert.equal(fresh.readPanelCollapsed(), true)
})

import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import {
  readCommsSelection,
  resetCommsSelection,
  restorableSelection,
  writeCommsSelection,
  type CommsSelection,
} from './comms-selection'

function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial))
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
      setItem: (key: string, value: string) => void map.set(key, value),
    },
  })
  return map
}

const rosters = (over: Partial<Parameters<typeof restorableSelection>[1]> = {}) => ({
  channelIds: ['chan-1', 'chan-2'],
  agentModels: ['hermes', 'atlas'],
  conversationIds: ['conv-4', 'conv-9'],
  ...over,
})

beforeEach(() => {
  vi.unstubAllGlobals()
  resetCommsSelection()
})

test('a channel selection round-trips', () => {
  installStorage()
  writeCommsSelection({ t: 'channel', id: 'chan-2' })
  resetCommsSelection()
  assert.deepEqual(readCommsSelection(), { t: 'channel', id: 'chan-2' })
})

test('an agent thread round-trips, thread id included', () => {
  installStorage()
  writeCommsSelection({ t: 'agent', model: 'hermes', conversationId: 'conv-4' })
  resetCommsSelection()
  assert.deepEqual(readCommsSelection(), { t: 'agent', model: 'hermes', conversationId: 'conv-4' })
})

test('an agent with no thread yet round-trips as a fresh thread', () => {
  installStorage()
  writeCommsSelection({ t: 'agent', model: 'hermes', conversationId: null })
  resetCommsSelection()
  assert.deepEqual(readCommsSelection(), { t: 'agent', model: 'hermes', conversationId: null })
})

test('nothing stored is no memory, and garbage is no memory either', () => {
  installStorage()
  assert.equal(readCommsSelection(), null)
  for (const raw of ['not json', '{}', '[]', '"a string"', '{"t":"channel"}', '{"t":"agent"}', '{"t":"nope","id":"x"}']) {
    resetCommsSelection()
    installStorage({ 'talaria:comms-selection': raw })
    assert.equal(readCommsSelection(), null, `"${raw}" should not restore`)
  }
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
  })
  writeCommsSelection({ t: 'channel', id: 'chan-2' })
  assert.deepEqual(readCommsSelection(), { t: 'channel', id: 'chan-2' })
})

// ── What may be restored ────────────────────────────────────────────────────

test('a remembered channel is restored when it still exists', () => {
  assert.deepEqual(restorableSelection({ t: 'channel', id: 'chan-2' }, rosters()), { t: 'channel', id: 'chan-2' })
})

test('a channel you no longer have is not restored', () => {
  // Better the first-channel default than dropping someone into a channel they
  // were removed from, or one that was archived out from under them.
  assert.equal(restorableSelection({ t: 'channel', id: 'chan-gone' }, rosters()), null)
})

test('a retired agent is not restored', () => {
  assert.equal(restorableSelection({ t: 'agent', model: 'retired', conversationId: 'conv-4' }, rosters()), null)
})

test('a deleted thread degrades to the agent, not to nothing', () => {
  // The agent is still the thing you were talking to; only the thread is gone.
  assert.deepEqual(restorableSelection({ t: 'agent', model: 'hermes', conversationId: 'conv-gone' }, rosters()), {
    t: 'agent',
    model: 'hermes',
    conversationId: null,
  })
})

test('a thread list that has not loaded is trusted, not used to reject', () => {
  // Validating against a list that merely has not arrived would discard a good
  // memory and fall through to the first channel — the same race in a new place.
  assert.deepEqual(
    restorableSelection({ t: 'agent', model: 'hermes', conversationId: 'conv-4' }, rosters({ conversationIds: null })),
    { t: 'agent', model: 'hermes', conversationId: 'conv-4' },
  )
})

test('no memory restores nothing', () => {
  assert.equal(restorableSelection(null, rosters()), null)
})

test('the value written is the value Comms derives from its own URL', () => {
  // Comms builds `sel` from its search params alone, so the stored shape has to
  // be exactly that — this is the contract the view depends on.
  installStorage()
  const fromUrl: CommsSelection = { t: 'agent', model: 'hermes', conversationId: 'conv-4' }
  writeCommsSelection(fromUrl)
  resetCommsSelection()
  assert.deepEqual(restorableSelection(readCommsSelection(), rosters()), fromUrl)
})

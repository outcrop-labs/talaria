import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import {
  commsSelectionFromPath,
  isCommsPath,
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

// ── the path IS the selection, and only Comms' paths are ────────────────────

test('a Comms path decodes to the selection it encodes', () => {
  assert.deepEqual(commsSelectionFromPath('/comms/channel/chan-1'), { t: 'channel', id: 'chan-1' })
  assert.deepEqual(commsSelectionFromPath('/comms/agent/hermes'), {
    t: 'agent',
    model: 'hermes',
    conversationId: null,
  })
  assert.deepEqual(commsSelectionFromPath('/comms/agent/hermes/conv-4'), {
    t: 'agent',
    model: 'hermes',
    conversationId: 'conv-4',
  })
  // Ids travel through the URL encoded; a channel id with a slash or a space
  // must come back out as it went in.
  assert.deepEqual(commsSelectionFromPath('/comms/channel/a%2Fb'), { t: 'channel', id: 'a/b' })
})

test('bare /comms has no selection, which is what asks for the default', () => {
  assert.equal(commsSelectionFromPath('/comms'), null)
  // A kind with no id is not half a selection.
  assert.equal(commsSelectionFromPath('/comms/channel'), null)
  assert.equal(commsSelectionFromPath('/comms/agent'), null)
})

test('THE NAV RAIL BUG: another view\'s path is never a Comms selection', () => {
  // The regression this guards. Comms stays mounted for a beat after you click
  // a nav rail item, so its effects run against the NEXT view's pathname. The
  // default-selection effect navigates, so if it reads "Comms with nothing
  // selected" there, it restores the remembered channel and drags you back —
  // undoing the click. It is a race against unmount, so it presented as a rail
  // that needed two or three clicks before it moved.
  //
  // Both halves matter. `isCommsPath` is what the effects bail on...
  for (const p of ['/agents', '/boards', '/knowledge', '/admin', '/', '/commsomething']) {
    assert.equal(isCommsPath(p), false, `${p} must not read as Comms`)
    assert.equal(commsSelectionFromPath(p), null, `${p} must not yield a selection`)
  }
  // ...and this is why the base check is not merely defensive: these paths have
  // a second segment that a naive parse reads as a Comms selection outright.
  assert.equal(commsSelectionFromPath('/plan/agent/hermes'), null)
  assert.equal(commsSelectionFromPath('/boards/channel/b-1'), null)
})

test('every Comms path counts as Comms, prefix matching included', () => {
  assert.equal(isCommsPath('/comms'), true)
  assert.equal(isCommsPath('/comms/channel/chan-1'), true)
  assert.equal(isCommsPath('/comms/agent/hermes/conv-4'), true)
})

import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import {
  clearDraft,
  commsDraftKey,
  draftStorageKey,
  draftTarget,
  listDrafts,
  newAgentDraftKey,
  readDraft,
  saveDraft,
} from './comms-drafts'

function installLocalStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial))
  vi.stubGlobal('window', {
    localStorage: {
      getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
      key: (i: number) => [...map.keys()][i] ?? null,
      get length() {
        return map.size
      },
    },
  })
  return map
}

beforeEach(() => {
  vi.unstubAllGlobals()
})

test('a draft saved for a conversation reads back, and clearing it (on send) removes it', () => {
  const store = installLocalStorage()
  saveDraft('u1', 'chan-1', 'half a thought')
  assert.equal(readDraft('u1', 'chan-1'), 'half a thought')
  assert.ok(store.has('comms-draft:u1:chan-1'), 'stored under comms-draft:{userId}:{key}')
  clearDraft('u1', 'chan-1')
  assert.equal(readDraft('u1', 'chan-1'), null)
  assert.equal(store.size, 0)
})

test('a whitespace-only draft is no draft — and it erases the one before it', () => {
  const store = installLocalStorage()
  saveDraft('u1', 'chan-1', '   \n\t ')
  assert.equal(readDraft('u1', 'chan-1'), null)
  assert.equal(store.size, 0)
  saveDraft('u1', 'chan-1', 'real')
  saveDraft('u1', 'chan-1', '  ')
  assert.equal(readDraft('u1', 'chan-1'), null, 'deleting the text deletes the draft')
})

test("one user's drafts are invisible under another user id", () => {
  installLocalStorage()
  saveDraft('alice', 'chan-1', "alice's words")
  saveDraft('bob', 'chan-2', "bob's words")
  assert.equal(readDraft('bob', 'chan-1'), null)
  assert.deepEqual(
    listDrafts('alice').map((d) => [d.key, d.text]),
    [['chan-1', "alice's words"]],
  )
  assert.deepEqual(
    listDrafts('bob').map((d) => [d.key, d.text]),
    [['chan-2', "bob's words"]],
  )
})

test('an agent:<model>:new draft round-trips and lists under its full key', () => {
  installLocalStorage()
  const key = newAgentDraftKey('hermes')
  assert.equal(key, 'agent:hermes:new')
  saveDraft('u1', key, 'ask hermes about the launch')
  assert.equal(readDraft('u1', key), 'ask hermes about the launch')
  assert.deepEqual(listDrafts('u1').map((d) => d.key), ['agent:hermes:new'])
  assert.equal(draftStorageKey('u1', key), 'comms-draft:u1:agent:hermes:new')
})

test('drafts list newest first, and garbage under the prefix is skipped', () => {
  installLocalStorage({ 'comms-draft:u1:junk': 'not json', 'other:key': '{}' })
  saveDraft('u1', 'a', 'older', 1_000)
  saveDraft('u1', 'b', 'newer', 2_000)
  assert.deepEqual(listDrafts('u1').map((d) => d.key), ['b', 'a'])
})

test('storage that throws degrades to no drafts, never a crash', () => {
  vi.stubGlobal('window', {
    localStorage: {
      getItem: () => {
        throw new Error('private mode')
      },
      setItem: () => {
        throw new Error('private mode')
      },
      removeItem: () => {
        throw new Error('private mode')
      },
      key: () => {
        throw new Error('private mode')
      },
      get length(): number {
        throw new Error('private mode')
      },
    },
  })
  saveDraft('u1', 'chan-1', 'x')
  assert.equal(readDraft('u1', 'chan-1'), null)
  assert.deepEqual(listDrafts('u1'), [])
})

test('the composer key follows the Comms selection', () => {
  assert.equal(commsDraftKey({ t: 'channel', id: 'chan-1' }), 'chan-1')
  assert.equal(commsDraftKey({ t: 'agent', model: 'hermes', conversationId: 'conv-4' }), 'conv-4')
  assert.equal(commsDraftKey({ t: 'agent', model: 'hermes', conversationId: null }), 'agent:hermes:new')
  assert.equal(commsDraftKey({ t: 'threads' }), null)
  assert.equal(commsDraftKey(null), null)
})

test('a draft key resolves to the conversation it belongs to', () => {
  const lookup = { channelIds: ['chan-1'], conversations: [{ id: 'conv-4', agentModel: 'hermes' }] }
  assert.equal(draftTarget('chan-1', lookup), '/comms/channel/chan-1')
  assert.equal(draftTarget('conv-4', lookup), '/comms/agent/hermes/conv-4')
  assert.equal(draftTarget('agent:hermes:new', lookup), '/comms/agent/hermes')
  assert.equal(draftTarget('gone', lookup), null)
})

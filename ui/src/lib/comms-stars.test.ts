import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { agentStarKey, channelStarKey, isStarred, parseStarKey, readStars, starsStorageKey, threadStarKey, toggleStar } from './comms-stars'

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

beforeEach(() => vi.unstubAllGlobals())

test('star keys are tagged strings, and the storage key is per user', () => {
  assert.equal(channelStarKey('c1'), 'channel:c1')
  assert.equal(agentStarKey('hermes'), 'agent:hermes')
  assert.equal(threadStarKey('t1'), 'thread:t1')
  assert.equal(starsStorageKey('u1'), 'talaria:comms-stars:u1')
})

test('nothing stored: nothing starred', () => {
  installLocalStorage()
  assert.deepEqual(readStars('u1'), [])
  assert.equal(isStarred('u1', 'channel:c1'), false)
})

test('toggling stars, then unstars, and returns the new list each time', () => {
  installLocalStorage()
  assert.deepEqual(toggleStar('u1', 'channel:c1'), ['channel:c1'])
  assert.equal(isStarred('u1', 'channel:c1'), true)
  assert.deepEqual(readStars('u1'), ['channel:c1'])
  assert.deepEqual(toggleStar('u1', 'channel:c1'), [])
  assert.equal(isStarred('u1', 'channel:c1'), false)
  assert.deepEqual(readStars('u1'), [])
})

test('stars keep the order they were starred in, newest last', () => {
  installLocalStorage()
  toggleStar('u1', 'agent:hermes')
  toggleStar('u1', 'channel:c2')
  toggleStar('u1', 'channel:c1')
  assert.deepEqual(readStars('u1'), ['agent:hermes', 'channel:c2', 'channel:c1'])
  // Unstarring from the middle keeps the rest in order; re-starring goes last.
  toggleStar('u1', 'channel:c2')
  toggleStar('u1', 'channel:c2')
  assert.deepEqual(readStars('u1'), ['agent:hermes', 'channel:c1', 'channel:c2'])
})

test("one person's stars are not another's", () => {
  const store = installLocalStorage()
  toggleStar('u1', 'channel:c1')
  assert.deepEqual(readStars('u2'), [])
  toggleStar('u2', 'agent:atlas')
  assert.deepEqual(readStars('u1'), ['channel:c1'])
  assert.deepEqual(readStars('u2'), ['agent:atlas'])
  assert.equal(store.get('talaria:comms-stars:u1'), '["channel:c1"]')
})

test('a malformed stored value reads as nothing starred', () => {
  const store = installLocalStorage()
  for (const raw of ['"nope"', '{"channel:c1":true}', 'not json', 'null', '42']) {
    store.set('talaria:comms-stars:u1', raw)
    assert.deepEqual(readStars('u1'), [], raw)
  }
  // And toggling over it starts clean rather than throwing.
  store.set('talaria:comms-stars:u1', '"nope"')
  assert.deepEqual(toggleStar('u1', 'channel:c1'), ['channel:c1'])
})

test('unknown key kinds are ignored — stored or toggled', () => {
  const store = installLocalStorage()
  store.set('talaria:comms-stars:u1', JSON.stringify(['channel:c1', 'board:b1', 7, '', 'channel:', 'agent:hermes', 'channel:c1']))
  assert.deepEqual(readStars('u1'), ['channel:c1', 'agent:hermes'], 'junk dropped, duplicates collapsed')
  assert.deepEqual(toggleStar('u1', 'board:b1'), ['channel:c1', 'agent:hermes'], 'an unknown kind does not star')
  assert.equal(isStarred('u1', 'board:b1'), false)
})

test('a pinned agent thread is a star like any other', () => {
  installLocalStorage()
  assert.deepEqual(toggleStar('u1', threadStarKey('t1')), ['thread:t1'])
  assert.equal(isStarred('u1', 'thread:t1'), true)
  assert.deepEqual(parseStarKey('thread:t1'), { kind: 'thread', id: 't1' })
  assert.deepEqual(toggleStar('u1', 'thread:t1'), [])
})

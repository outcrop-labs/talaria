import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { HttpError } from './fetch-json'
import {
  agentStarKey,
  channelStarKey,
  parseStarKey,
  readStars,
  starsStorageKey,
  starsToKeep,
  threadStarKey,
  uploadOutcome,
  type StarKey,
} from './comms-stars'

// The pins themselves live on the server now (comms-stars.svelte.ts reads
// and writes /api/me/pins). What stays here is the key grammar, the reader
// for the copy older builds kept in localStorage, and the rule that decides
// which of that copy survives an upload pass.

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
  assert.deepEqual(parseStarKey('thread:t1'), { kind: 'thread', id: 't1' })
  assert.equal(parseStarKey('board:b1'), null)
})

test('nothing stored: nothing to fold in', () => {
  installLocalStorage()
  assert.deepEqual(readStars('u1'), [])
})

test("the legacy copy reads in star order, per person, and one person's is not another's", () => {
  const store = installLocalStorage()
  store.set('talaria:comms-stars:u1', JSON.stringify(['agent:hermes', 'channel:c2', 'thread:t1']))
  assert.deepEqual(readStars('u1'), ['agent:hermes', 'channel:c2', 'thread:t1'])
  assert.deepEqual(readStars('u2'), [])
})

test('a malformed stored value reads as nothing, and junk keys are dropped', () => {
  const store = installLocalStorage()
  for (const raw of ['"nope"', '{"channel:c1":true}', 'not json', 'null', '42']) {
    store.set('talaria:comms-stars:u1', raw)
    assert.deepEqual(readStars('u1'), [], raw)
  }
  store.set('talaria:comms-stars:u1', JSON.stringify(['channel:c1', 'board:b1', 7, '', 'channel:', 'agent:hermes', 'channel:c1']))
  assert.deepEqual(readStars('u1'), ['channel:c1', 'agent:hermes'], 'junk dropped, duplicates collapsed')
})

test('an upload that will never succeed is refused; one that might is retried', () => {
  // Refused for good: the server will give the same answer every time.
  for (const status of [400, 404, 409, 413, 422]) assert.equal(uploadOutcome(new HttpError(status, 'x')), 'refused', String(status))
  // Worth another try: a restart, a lapsed session, throttling, no answer.
  for (const status of [500, 502, 503, 401, 403, 408, 429]) assert.equal(uploadOutcome(new HttpError(status, 'x')), 'retry', String(status))
  assert.equal(uploadOutcome(new TypeError('Failed to fetch')), 'retry')
  assert.equal(uploadOutcome('offline'), 'retry')
})

test('the local copy keeps only what is worth another try, and clears when nothing is', () => {
  const r = (key: StarKey, outcome: 'landed' | 'refused' | 'retry') => ({ key, outcome })
  // The deploy-moment 502 and the expired session: those pins stay.
  assert.deepEqual(
    starsToKeep([r('channel:c1', 'landed'), r('agent:a', 'retry'), r('thread:t', 'refused'), r('channel:c2', 'retry')]),
    ['agent:a', 'channel:c2'],
  )
  // Everything landed or was refused for good: nothing left to keep.
  assert.deepEqual(starsToKeep([r('channel:c1', 'landed'), r('thread:t', 'refused')]), [])
  assert.deepEqual(starsToKeep([]), [])
})

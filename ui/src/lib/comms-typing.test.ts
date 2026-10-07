import assert from 'node:assert/strict'
import { afterEach, beforeEach, test, vi } from 'vitest'

const posts: Array<{ url: string; body: unknown }> = []
vi.mock('./fetch-json', () => ({
  postJson: (url: string, body: unknown) => {
    posts.push({ url, body })
    return Promise.resolve({ ok: true })
  },
}))

const { setTyping, typersIn, typingSentence, createTypingSender, typingKey } = await import('./comms-typing.svelte')

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-10-05T12:00:00Z'))
  posts.length = 0
})
afterEach(() => vi.useRealTimers())

test('a typing signal lists the typer — never yourself — until it stops', () => {
  setTyping('c1', 'maya', true)
  setTyping('c1', 'me', true)
  assert.deepEqual(typersIn('c1', 'me'), ['maya'])
  setTyping('c1', 'maya', false)
  assert.deepEqual(typersIn('c1', 'me'), [])
  setTyping('c1', 'me', false)
})

test('a typing signal that is never stopped expires after six seconds', () => {
  setTyping('c2', 'jordan', true)
  vi.advanceTimersByTime(5000)
  assert.deepEqual(typersIn('c2', null), ['jordan'])
  vi.advanceTimersByTime(1500)
  assert.deepEqual(typersIn('c2', null), [])
})

test('typersIn without a channel is nobody', () => {
  assert.deepEqual(typersIn(null, 'me'), [])
})

test('the sentence names one or two people, then says several', () => {
  assert.equal(typingSentence([]), '')
  assert.equal(typingSentence(['Maya']), 'Maya is typing')
  assert.equal(typingSentence(['Maya', 'Jordan']), 'Maya and Jordan are typing')
  assert.equal(typingSentence(['Maya', 'Jordan', 'Priya']), 'Several people are typing')
})

test('the sender pings at most every three seconds, and stops once', () => {
  const sender = createTypingSender(() => 'c3')
  sender.input(false)
  sender.input(false)
  vi.advanceTimersByTime(2000)
  sender.input(false)
  assert.equal(posts.length, 1, 'one ping inside the window')
  vi.advanceTimersByTime(1500)
  sender.input(false)
  assert.equal(posts.length, 2, 'a second ping after three seconds')
  sender.stop()
  sender.stop()
  assert.deepEqual(
    posts.map((p) => p.body),
    [{ typing: true }, { typing: true }, { typing: false }],
  )
  assert.equal(posts[0]?.url, '/api/channels/c3/typing')
})

test('emptying the composer stops the signal; switching channels stops the old one', () => {
  let channel = 'c4'
  const sender = createTypingSender(() => channel)
  sender.input(false)
  sender.input(true)
  channel = 'c5'
  sender.input(false)
  channel = 'c6'
  sender.input(false)
  assert.deepEqual(
    posts.map((p) => [p.url.split('/')[3], (p.body as { typing: boolean }).typing]),
    [
      ['c4', true],
      ['c4', false],
      ['c5', true],
      ['c5', false],
      ['c6', true],
    ],
  )
})

test('a thread is its own typing scope, apart from its channel', () => {
  assert.equal(typingKey('c7'), 'c7')
  assert.equal(typingKey('c7', 'r1'), 'c7#r1')
  setTyping(typingKey('c7', 'r1'), 'maya', true)
  assert.deepEqual(typersIn('c7', null), [], 'the channel line does not show thread typing')
  assert.deepEqual(typersIn(typingKey('c7', 'r1'), null), ['maya'])
  setTyping(typingKey('c7', 'r1'), 'maya', false)
})

test('a thread composer names its thread, and switching threads stops the old one', () => {
  let root = 'r1'
  const sender = createTypingSender(
    () => 'c8',
    () => root,
  )
  sender.input(false)
  root = 'r2'
  sender.input(false)
  assert.deepEqual(
    posts.map((p) => p.body),
    [
      { typing: true, threadRootId: 'r1' },
      { typing: false, threadRootId: 'r1' },
      { typing: true, threadRootId: 'r2' },
    ],
  )
})

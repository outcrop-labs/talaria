import assert from 'node:assert/strict'
import { test } from 'vitest'
import { CHURN_MS, SETTLE_MS, fenceOpen, reveal, revealedText, settling, standIn, trackArrivals, unsettledCount } from './stream-reveal'

// The library is pure on purpose — (text, arrivals, now) -> state — so the
// whole behaviour asserts here rather than being eyeballed against a live
// model. The interesting states (a character mid-churn, a fence just opened)
// are the ones hardest to reproduce by hand, which is why the renderer's
// markdown safety lives in the library and gets tested here.

test('whitespace never scrambles', () => {
  const text = 'a b\nc'
  const arrivals = [0, 0, 0, 0, 0]
  const out = reveal(text, arrivals, 10)
  assert.equal(out[1]!.ch, ' ')
  assert.equal(out[1]!.settled, 1)
  assert.equal(out[3]!.ch, '\n')
  assert.equal(out[3]!.settled, 1)
})

test('history does not animate: no arrival means long settled', () => {
  const out = reveal('hello', [], 1000)
  assert.ok(out.every((c) => c.settled === 1))
})

test('a fresh character churns at the churn rate and settles at the settle', () => {
  const at = 1000
  assert.ok(reveal('x', [at], at + 1)[0]!.settled < 1)
  // Determinism is the contract: same inputs, same frame.
  assert.equal(standIn(0, 0), standIn(0, 0))
  assert.equal(reveal('x', [at], at + CHURN_MS)[0]!.ch, standIn(0, 1))
  assert.equal(reveal('x', [at], at + SETTLE_MS)[0]!.settled, 1)
})

test('the tail shimmers, it does not strobe: columns flip on their own clocks', () => {
  // A column holds its glyph until its own period completes...
  assert.equal(standIn(7, 2), standIn(7, 3))
  assert.equal(standIn(7, 3), standIn(7, 4))
  // ...and flips on its own tick, not on the global one.
  assert.notEqual(standIn(7, 4), standIn(7, 5))
  // A shared period would re-roll the whole run in one synchronized flash;
  // the per-column periods and phases must keep peak simultaneous flip well
  // below the run's length. (With the current rhythm the peak in a 12-char
  // tail is 4 — a strobe would be 12.)
  let worst = 0
  for (let t = 0; t < 48; t++) {
    let changed = 0
    for (let i = 0; i < 12; i++) {
      if (standIn(i, t) !== standIn(i, t + 1)) changed++
    }
    worst = Math.max(worst, changed)
  }
  assert.ok(worst <= 4, `peak simultaneous re-rolls in a 12-char tail: ${worst}`)
})

test('trackArrivals stamps only growth', () => {
  const a = trackArrivals('', 'abc', [], 100)
  assert.deepEqual(a, [100, 100, 100])
  const b = trackArrivals('abc', 'abcd', a, 200)
  assert.deepEqual(b, [100, 100, 100, 200])
  // A rewrite is not an arrival.
  assert.deepEqual(trackArrivals('abc', 'xyz', a, 300), [])
  // No growth returns the record unchanged.
  assert.equal(trackArrivals('abc', 'abc', a, 400), a)
})

test('settling is true only while the recent tail is unresolved', () => {
  const arrivals = [0, 0, 0, 1000]
  assert.equal(settling('abcd', arrivals, 1000 + SETTLE_MS - 1), true)
  assert.equal(settling('abcd', arrivals, 1000 + SETTLE_MS), false)
  // Old arrivals beyond the window do not keep the clock alive.
  assert.equal(settling('a'.repeat(100) + 'b', [...Array(100).fill(0), 0], 5000), false)
})

test('fenceOpen tracks backtick and tilde fences', () => {
  assert.equal(fenceOpen('no fence'), false)
  assert.equal(fenceOpen('```\ncode'), true)
  assert.equal(fenceOpen('```\ncode\n```'), false)
  assert.equal(fenceOpen('~~~\ncode'), true)
  assert.equal(fenceOpen('   ```\ncode'), true) // GFM indents a fence up to 3 spaces
  assert.equal(fenceOpen('inline `ticks` are not a fence'), false)
})

// The renderer's contract: a stand-in must never be parsed as syntax. The
// test pins the FULL markdown-active set, not the current intersection with
// the glyph pool, so a new syntax glyph entering the pool fails here until
// the escaper covers it.
const SYNTAX = new Set(['*', '_', '`', '[', ']', '<', '>', '\\', '|', '~', '#', '!'])

test('revealedText escapes markdown-active stand-ins, and only them', () => {
  const text = 'abcd'
  const at = 1000
  // Ticks 0–2: 2*CHURN_MS + 1 is still inside SETTLE_MS, so the characters
  // are unsettled for every frame asserted below.
  for (const tick of [0, 1, 2]) {
    const now = at + tick * CHURN_MS + 1
    const out = revealedText(text, [at, at, at, at], now)
    let expected = ''
    for (let i = 0; i < text.length; i++) {
      const g = standIn(i, tick)
      expected += SYNTAX.has(g) ? '\\' + g : g
    }
    assert.equal(out, expected, `tick ${tick}`)
  }
})

test('revealedText does not escape inside an open fence', () => {
  const text = '```\nab'
  const at = 1000
  const out = revealedText(text, [0, 0, 0, 0, at, at], at + 1)
  assert.ok(!out.includes('\\'), `a backslash printed inside a fence: ${out}`)
  assert.ok(out.startsWith('```\n'))
})

test('revealedText returns the input unchanged when nothing is unsettled', () => {
  const text = 'settled text'
  const arrivals = text.split('').map(() => 0)
  assert.equal(revealedText(text, arrivals, 10_000), text)
})

test('unsettledCount counts only the trailing run, and skips whitespace without stopping', () => {
  const text = 'abcd'
  assert.equal(unsettledCount(text, [0, 0, 0, 0], 1000), 0)
  assert.equal(unsettledCount(text, [0, 0, 0, 1000], 1010), 1)
  assert.equal(unsettledCount(text, [0, 0, 1000, 1000], 1010), 2)
  // A settled character ends the run even with fresh ones further back is
  // impossible (arrivals are monotonic) — what it DOES stop is the scan.
  assert.equal(unsettledCount(text, [0, 0, 0, 1000], 1000 + SETTLE_MS), 0)
  // Whitespace in the tail never counts, but does not hide what is past it:
  // a fresh chunk 'Z ' carries both at the same timestamp, and a break on the
  // trailing space would have reported 0.
  assert.equal(unsettledCount('abZ ', [0, 0, 1000, 1000], 1010), 1)
  assert.equal(unsettledCount('ab c', [0, 0, 0, 1000], 1010), 1)
  // No arrivals at all is history — nothing unsettled, no matter the clock.
  assert.equal(unsettledCount('abcdefgh', [], 5000), 0)
})

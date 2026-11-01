import assert from 'node:assert/strict'
import { afterEach, beforeEach, test, vi } from 'vitest'
import { BADGE_DEBOUNCE_MS, createBadgeReporter } from './desktop-badge'

let sent: number[] = []
beforeEach(() => {
  vi.useFakeTimers()
  sent = []
})
afterEach(() => vi.useRealTimers())

const reporter = () => createBadgeReporter((n) => sent.push(n))

test('a burst is one report, of the count it settled on', () => {
  const r = reporter()
  r.update(2, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [2])
  // 2 → 3 → 2 inside the window: the pending 3 is dropped, nothing is sent.
  r.update(3, false)
  vi.advanceTimersByTime(100)
  r.update(2, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS * 2)
  assert.deepEqual(sent, [2])
  r.update(1, false)
  r.update(4, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [2, 4])
})

test('nothing is reported before the first read, and an unchanged count is not re-sent', () => {
  const r = reporter()
  r.update(undefined, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [])
  r.update(5, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  r.update(5, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [5])
})

test('a signed-out page reports 0, whatever the cache still holds', () => {
  const r = reporter()
  r.update(3, false)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  r.update(3, true)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [3, 0])
  // Before any read at all, too: the login screen's own switcher.
  const fresh = reporter()
  fresh.update(undefined, true)
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [3, 0, 0])
})

test('stop cancels a pending report', () => {
  const r = reporter()
  r.update(7, false)
  r.stop()
  vi.advanceTimersByTime(BADGE_DEBOUNCE_MS)
  assert.deepEqual(sent, [])
})

import assert from 'node:assert/strict'
import { afterEach, beforeEach, test, vi } from 'vitest'
import { SESSION_HOLD_GRACE_MS, startHoldGrace } from './session-hold'

// The grace boundary, not the wait itself. The hold is honest for one
// round-trip — the timer exists so that past that, the gate stops being a
// dead screen (GH #327's residual: a wedged session read held a frame with
// zero interactive elements for the read deadline plus the query retry). The
// behaviours that make that true: quiet under grace, loud exactly once at the
// boundary, disarm when the wait resolves, re-arm from zero for a new wait.

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

test('quiet under the grace — one honest round-trip is not a stuck screen', () => {
  let fired = 0
  startHoldGrace(() => true, () => fired++)
  vi.advanceTimersByTime(SESSION_HOLD_GRACE_MS - 1)
  assert.equal(fired, 0)
})

test('fires once at the boundary', () => {
  let fired = 0
  startHoldGrace(() => true, () => fired++)
  vi.advanceTimersByTime(SESSION_HOLD_GRACE_MS)
  assert.equal(fired, 1)
  vi.advanceTimersByTime(SESSION_HOLD_GRACE_MS)
  assert.equal(fired, 1, 'onGrace fires exactly once per pending period')
})

test('not pending: nothing is armed, the stop is inert', () => {
  let fired = 0
  const stop = startHoldGrace(() => false, () => fired++)
  vi.advanceTimersByTime(10 * SESSION_HOLD_GRACE_MS)
  assert.equal(fired, 0)
  stop() // must not throw on an unarmed stop
  assert.equal(fired, 0)
})

test('disarms when the pending period ends early', () => {
  let fired = 0
  const stop = startHoldGrace(() => true, () => fired++)
  vi.advanceTimersByTime(SESSION_HOLD_GRACE_MS / 2)
  stop()
  vi.advanceTimersByTime(10 * SESSION_HOLD_GRACE_MS)
  assert.equal(fired, 0)
})

test('re-arms from zero for a new pending period', () => {
  let fired = 0
  const first = startHoldGrace(() => true, () => fired++)
  vi.advanceTimersByTime(SESSION_HOLD_GRACE_MS / 2)
  first()
  // A resolved wait disarmed the first grace; a new one starts fresh — the
  // person gets the full quiet round-trip again, not the tail of a timer that
  // was armed against the last wedge.
  const second = startHoldGrace(() => true, () => fired++)
  vi.advanceTimersByTime(SESSION_HOLD_GRACE_MS - 1)
  assert.equal(fired, 0)
  vi.advanceTimersByTime(1)
  assert.equal(fired, 1)
  second()
})

test('a custom grace is honored — the boundary is the caller’s, not a constant', () => {
  let fired = 0
  const stop = startHoldGrace(() => true, () => fired++, 250)
  vi.advanceTimersByTime(249)
  assert.equal(fired, 0)
  vi.advanceTimersByTime(1)
  assert.equal(fired, 1)
  stop()
})

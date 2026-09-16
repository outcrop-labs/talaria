import { afterEach, describe, expect, it, vi } from 'vitest'
import { scheduleIdleWarmup } from './idle-warmup'

// The point of scheduleIdleWarmup is that the ABSENT global is read through
// `typeof`, never by name — WebKit has no requestIdleCallback, and a bare
// reference throws ReferenceError before any `??` fallback runs. Both tests
// install exactly the globals the branch under test needs, so the other
// branch's global is absent the way it is on real engines.
const g = globalThis as Record<string, unknown>

const stubIdle = () => {
  g.requestIdleCallback = vi.fn((cb: () => void) => {
    cb()
    return 7
  }) as unknown as typeof requestIdleCallback
  g.cancelIdleCallback = vi.fn() as unknown as typeof cancelIdleCallback
}

afterEach(() => {
  delete g.requestIdleCallback
  delete g.cancelIdleCallback
  vi.restoreAllMocks()
})

describe('scheduleIdleWarmup', () => {
  it('uses requestIdleCallback where the platform ships it, and cancelIdleCallback on cleanup', async () => {
    stubIdle()
    const setTimeoutSpy = vi.spyOn(globalThis, 'setTimeout')
    const task = vi.fn(() => Promise.resolve())
    const cancel = scheduleIdleWarmup(task)
    await vi.waitFor(() => expect(task).toHaveBeenCalledTimes(1))
    // vi.waitFor itself polls via setTimeout(…, 0) — assert no call CARRIED
    // the 250ms fallback delay rather than that setTimeout went untouched.
    expect(setTimeoutSpy).not.toHaveBeenCalledWith(expect.any(Function), 250)
    cancel()
    expect(g.cancelIdleCallback).toHaveBeenCalledWith(7)
  })

  it('falls back to setTimeout where requestIdleCallback is absent (WebKit), reading it through typeof without throwing', async () => {
    // No requestIdleCallback installed — the branch must detect that via
    // `typeof`, not trip over the missing global.
    const setTimeoutSpy = vi
      .spyOn(globalThis, 'setTimeout')
      .mockImplementation(((cb: () => void) => {
        cb()
        return 3
      }) as unknown as typeof setTimeout)
    const clearTimeoutSpy = vi.spyOn(globalThis, 'clearTimeout')
    const task = vi.fn(() => Promise.resolve())
    const cancel = scheduleIdleWarmup(task)
    await vi.waitFor(() => expect(task).toHaveBeenCalledTimes(1))
    expect(setTimeoutSpy).toHaveBeenCalledWith(expect.any(Function), 250)
    cancel()
    expect(clearTimeoutSpy).toHaveBeenCalledWith(3)
  })

  it('swallows a rejecting task instead of raising an unhandled rejection', async () => {
    stubIdle()
    const task = vi.fn(() => Promise.reject(new Error('chunk load failed')))
    scheduleIdleWarmup(task)
    await vi.waitFor(() => expect(task).toHaveBeenCalledTimes(1))
    // If the rejection escaped, vitest fails the suite on unhandled rejection
    // — reaching this line means the catch held.
  })
})

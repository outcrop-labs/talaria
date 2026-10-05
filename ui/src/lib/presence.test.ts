import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PRESENCE_INTERVAL_MS, startPresenceHeartbeat, type VisibilitySource } from './presence.svelte'

// A stand-in for `document`: the test env is node (no DOM), so the heartbeat
// takes its visibility source as an injected dependency.
function fakeDoc(initial: DocumentVisibilityState = 'visible') {
  const listeners = new Set<() => void>()
  const doc: VisibilitySource & { set: (v: DocumentVisibilityState) => void; listenerCount: () => number } = {
    visibilityState: initial,
    addEventListener: (_type: 'visibilitychange', fn: () => void) => void listeners.add(fn),
    removeEventListener: (_type: 'visibilitychange', fn: () => void) => void listeners.delete(fn),
    set(v) {
      doc.visibilityState = v
      for (const fn of [...listeners]) fn()
    },
    listenerCount: () => listeners.size,
  }
  return doc
}

describe('startPresenceHeartbeat', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('pings once on mount when visible', () => {
    const ping = vi.fn(() => Promise.resolve())
    const stop = startPresenceHeartbeat({ doc: fakeDoc(), ping })
    expect(ping).toHaveBeenCalledTimes(1)
    stop()
  })

  it('pings every 30s while visible', () => {
    expect(PRESENCE_INTERVAL_MS).toBe(30_000)
    const ping = vi.fn(() => Promise.resolve())
    const stop = startPresenceHeartbeat({ doc: fakeDoc(), ping })
    vi.advanceTimersByTime(30_000)
    expect(ping).toHaveBeenCalledTimes(2)
    vi.advanceTimersByTime(60_000)
    expect(ping).toHaveBeenCalledTimes(4)
    stop()
  })

  it('skips pings while hidden, including on mount', () => {
    const doc = fakeDoc('hidden')
    const ping = vi.fn(() => Promise.resolve())
    const stop = startPresenceHeartbeat({ doc, ping })
    vi.advanceTimersByTime(120_000)
    expect(ping).not.toHaveBeenCalled()
    stop()
  })

  it('pings once on becoming visible', () => {
    const doc = fakeDoc('hidden')
    const ping = vi.fn(() => Promise.resolve())
    const stop = startPresenceHeartbeat({ doc, ping })
    doc.set('visible')
    expect(ping).toHaveBeenCalledTimes(1)
    // Going hidden again is not a ping.
    doc.set('hidden')
    expect(ping).toHaveBeenCalledTimes(1)
    stop()
  })

  it('keeps pinging while hidden when visibleOnly is false', () => {
    const doc = fakeDoc('hidden')
    const ping = vi.fn(() => Promise.resolve())
    const stop = startPresenceHeartbeat({ doc, ping, visibleOnly: false })
    expect(ping).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(30_000)
    expect(ping).toHaveBeenCalledTimes(2)
    stop()
  })

  it('swallows ping failures', async () => {
    const ping = vi.fn(() => Promise.reject(new Error('down')))
    const stop = startPresenceHeartbeat({ doc: fakeDoc(), ping })
    vi.advanceTimersByTime(30_000)
    await vi.runAllTicks()
    expect(ping).toHaveBeenCalledTimes(2)
    stop()
  })

  it('cleanup stops the interval and the visibility listener', () => {
    const doc = fakeDoc()
    const ping = vi.fn(() => Promise.resolve())
    const stop = startPresenceHeartbeat({ doc, ping })
    stop()
    expect(doc.listenerCount()).toBe(0)
    vi.advanceTimersByTime(120_000)
    doc.set('hidden')
    doc.set('visible')
    expect(ping).toHaveBeenCalledTimes(1)
  })
})

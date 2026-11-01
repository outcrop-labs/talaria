import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PRESENCE_INTERVAL_MS, startPresenceHeartbeat, type VisibilitySource } from './presence.svelte'
import { noteInput, resetInputForTests } from './looking'
import type { PresenceBody } from './notify-wire'

// A stand-in for `document`: the test env is node (no DOM), so the heartbeat
// takes its visibility source as an injected dependency.
function fakeDoc(initial: DocumentVisibilityState = 'visible', focused = true) {
  const listeners = new Set<() => void>()
  const doc: VisibilitySource & {
    focused: boolean
    set: (v: DocumentVisibilityState) => void
    listenerCount: () => number
  } = {
    visibilityState: initial,
    focused,
    hasFocus: () => doc.focused,
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

const NOW0 = 1_800_000_000_000

describe('startPresenceHeartbeat — the I-17 body', () => {
  let clock = NOW0
  const now = () => clock
  beforeEach(() => {
    vi.useFakeTimers()
    clock = NOW0
    resetInputForTests()
  })
  afterEach(() => {
    vi.useRealTimers()
    resetInputForTests()
  })
  const advance = (ms: number) => {
    clock += ms
    vi.advanceTimersByTime(ms)
  }
  const start = (doc = fakeDoc(), kind: 'web' | 'desktop' = 'web') => {
    const sent: PresenceBody[] = []
    const send = vi.fn(async (b: PresenceBody) => void sent.push(b))
    const stop = startPresenceHeartbeat({ doc, send, win: new EventTarget(), now, clientId: 'tab-test-0001', kind, osIdle: null })
    return { sent, send, stop, doc }
  }

  it('beats at once, with a body (the no-body ping is gone)', () => {
    noteInput(NOW0)
    const { sent, stop } = start()
    expect(sent).toEqual([{ clientId: 'tab-test-0001', kind: 'web', state: 'looking' }])
    stop()
  })

  it('beats every 30 s whether or not anything changed', () => {
    expect(PRESENCE_INTERVAL_MS).toBe(30_000)
    const { sent, stop } = start(fakeDoc('hidden'))
    advance(30_000)
    advance(30_000)
    expect(sent.map((b) => b.state)).toEqual(['hidden', 'hidden', 'hidden'])
    stop()
  })

  it('goes looking → active → idle as the last input ages, and hidden when the tab is', () => {
    noteInput(NOW0)
    const doc = fakeDoc()
    const { sent, stop } = start(doc)
    advance(65_000) // input 65 s ago: active
    expect(sent.at(-1)?.state).toBe('active')
    advance(60_000) // 125 s: idle
    expect(sent.at(-1)?.state).toBe('idle')
    doc.set('hidden')
    advance(5_000)
    expect(sent.at(-1)?.state).toBe('hidden')
    expect(sent.map((b) => b.state)).toEqual(['looking', 'active', 'idle', 'hidden'])
    stop()
  })

  it('a change sends at once, but never two beats inside 5 s', () => {
    noteInput(NOW0)
    const doc = fakeDoc()
    const { sent, stop } = start(doc)
    doc.set('hidden') // 0 s after the first beat: held
    expect(sent).toHaveLength(1)
    advance(5_000)
    expect(sent.map((b) => b.state)).toEqual(['looking', 'hidden'])
    stop()
  })

  it('input on an idle tab makes it looking on the next beat', () => {
    const doc = fakeDoc()
    const { sent, stop } = start(doc)
    expect(sent[0]?.state).toBe('idle') // never touched
    advance(6_000)
    noteInput(clock)
    advance(1)
    expect(sent.at(-1)?.state).toBe('looking')
    stop()
  })

  it('swallows send failures', () => {
    const send = vi.fn(() => Promise.reject(new Error('down')))
    const stop = startPresenceHeartbeat({ doc: fakeDoc(), send, win: new EventTarget(), now, clientId: 'tab-test-0001', osIdle: null })
    advance(30_000)
    expect(send).toHaveBeenCalledTimes(2)
    stop()
  })

  it('cleanup stops the interval and the visibility listener', () => {
    const doc = fakeDoc()
    const { send, stop } = start(doc)
    expect(doc.listenerCount()).toBe(1)
    stop()
    expect(doc.listenerCount()).toBe(0)
    advance(120_000)
    expect(send).toHaveBeenCalledTimes(1)
  })

  it('inside the shell: kind desktop and osIdleMs, and OS input keeps it active with the window hidden', async () => {
    const sent: PresenceBody[] = []
    const stop = startPresenceHeartbeat({
      doc: fakeDoc('hidden', false),
      send: async (b) => void sent.push(b),
      win: new EventTarget(),
      now,
      clientId: 'tab-test-0002',
      kind: 'desktop',
      osIdle: async () => 4_000,
    })
    await vi.advanceTimersByTimeAsync(0)
    expect(sent[0]).toEqual({ clientId: 'tab-test-0002', kind: 'desktop', state: 'active', osIdleMs: 4000 })
    stop()
  })
})

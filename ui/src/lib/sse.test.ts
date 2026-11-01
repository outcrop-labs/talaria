import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { HIDDEN_PAUSE_H1_MS, HIDDEN_PAUSE_MS, PROBE_AFTER, STABLE_MS, backoffDelay, hiddenPauseMs, openStream, type StreamState } from './sse'

class FakeES {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSED = 2
  static all: FakeES[] = []
  readyState = FakeES.CONNECTING
  onmessage: ((e: { data: string }) => void) | null = null
  onopen: (() => void) | null = null
  onerror: (() => void) | null = null
  closed = false
  constructor(public url: string) {
    FakeES.all.push(this)
  }
  open() {
    this.readyState = FakeES.OPEN
    this.onopen?.()
  }
  /** A non-200 (502, 524): the browser gives up. */
  fail() {
    this.readyState = FakeES.CLOSED
    this.onerror?.()
  }
  close() {
    this.closed = true
    this.readyState = FakeES.CLOSED
  }
}

const listeners = () => {
  const map = new Map<string, Set<() => void>>()
  return {
    addEventListener: (t: string, fn: () => void) => void (map.get(t) ?? map.set(t, new Set()).get(t)!).add(fn),
    removeEventListener: (t: string, fn: () => void) => void map.get(t)?.delete(fn),
    fire: (t: string) => map.get(t)?.forEach((fn) => fn()),
    count: (t: string) => map.get(t)?.size ?? 0,
  }
}

const at = (i: number): FakeES => {
  const es = FakeES.all[i]
  if (!es) throw new Error(`no EventSource #${i}`)
  return es
}

describe('backoffDelay', () => {
  it('doubles from one second to a thirty-second cap', () => {
    const mid = () => 0.5
    expect([1, 2, 3, 4, 5, 6, 7].map((n) => backoffDelay(n, mid))).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000])
  })
  it('jitters within ±20 %', () => {
    expect(backoffDelay(1, () => 0)).toBe(800)
    expect(backoffDelay(1, () => 0.9999999)).toBe(1200)
    expect(backoffDelay(3, () => 0)).toBe(3200)
  })
})

describe('openStream', () => {
  let win: ReturnType<typeof listeners>
  let doc: ReturnType<typeof listeners> & { visibilityState: string }
  const fetchMock = vi.fn()

  beforeEach(() => {
    vi.useFakeTimers()
    FakeES.all = []
    win = listeners()
    doc = Object.assign(listeners(), { visibilityState: 'visible' })
    vi.stubGlobal('EventSource', FakeES)
    vi.stubGlobal('window', win)
    vi.stubGlobal('document', doc)
    vi.stubGlobal('fetch', fetchMock)
    vi.spyOn(Math, 'random').mockReturnValue(0.5)
    fetchMock.mockReset()
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('without reconnect, is the plain door it always was', () => {
    const got: string[] = []
    const stop = openStream('/x', (d) => got.push(d))
    const es = at(0)
    es.onmessage?.({ data: 'a' })
    expect(got).toEqual(['a'])
    es.fail()
    vi.advanceTimersByTime(60_000)
    expect(FakeES.all).toHaveLength(1)
    expect(win.count('online')).toBe(0)
    stop()
    expect(es.closed).toBe(true)
  })

  it('reopens a CLOSED stream at 1 s, 2 s, 4 s and calls onOpen on every open', () => {
    const opens: boolean[] = []
    const stop = openStream('/api/me/events', () => {}, { reconnect: true, onOpen: ({ first }) => opens.push(first) })
    at(0).open()
    for (const [i, wait] of [1000, 2000, 4000].entries()) {
      at(i).fail()
      vi.advanceTimersByTime(wait - 1)
      expect(FakeES.all).toHaveLength(i + 1)
      vi.advanceTimersByTime(1)
      expect(FakeES.all).toHaveLength(i + 2)
    }
    at(3).open()
    expect(opens).toEqual([true, false])
    stop()
    expect(at(3).closed).toBe(true)
  })

  it("leaves the browser's own retry alone (CONNECTING is not CLOSED)", () => {
    const stop = openStream('/x', () => {}, { reconnect: true })
    const es = at(0)
    es.readyState = FakeES.CONNECTING
    es.onerror?.()
    vi.advanceTimersByTime(60_000)
    expect(FakeES.all).toHaveLength(1)
    stop()
  })

  it('resets the backoff once a connection held for 30 s', () => {
    const stop = openStream('/x', () => {}, { reconnect: true })
    at(0).fail()
    vi.advanceTimersByTime(1000)
    at(1).fail()
    vi.advanceTimersByTime(2000)
    at(2).open()
    vi.advanceTimersByTime(STABLE_MS)
    at(2).fail()
    vi.advanceTimersByTime(1000)
    expect(FakeES.all).toHaveLength(4)
    stop()
  })

  it('reconnects at once when the network or the tab comes back', () => {
    const stop = openStream('/x', () => {}, { reconnect: true })
    at(0).fail()
    win.fire('online')
    expect(FakeES.all).toHaveLength(2)
    at(1).fail()
    doc.visibilityState = 'hidden'
    doc.fire('visibilitychange')
    expect(FakeES.all).toHaveLength(2)
    doc.visibilityState = 'visible'
    doc.fire('visibilitychange')
    expect(FakeES.all).toHaveLength(3)
    // An open stream is not kicked.
    at(2).open()
    win.fire('online')
    expect(FakeES.all).toHaveLength(3)
    stop()
    expect(win.count('online')).toBe(0)
    expect(doc.count('visibilitychange')).toBe(0)
  })

  it('after five failures asks for the session, and stops when signed out', async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ user: null }), { status: 200, headers: { 'content-type': 'application/json' } }))
    const stop = openStream('/x', () => {}, { reconnect: true })
    for (let i = 0; i < PROBE_AFTER; i++) {
      at(i).fail()
      if (i < PROBE_AFTER - 1) vi.advanceTimersByTime(backoffDelay(i + 1))
    }
    expect(FakeES.all).toHaveLength(PROBE_AFTER)
    await vi.advanceTimersByTimeAsync(backoffDelay(PROBE_AFTER))
    expect(String(fetchMock.mock.calls[0]?.[0])).toContain('/api/auth/session')
    expect(FakeES.all).toHaveLength(PROBE_AFTER)
    await vi.advanceTimersByTimeAsync(120_000)
    expect(FakeES.all).toHaveLength(PROBE_AFTER)
    stop()
  })

  it('a kick during the session probe opens ONE source, not two', async () => {
    // The probe answers "signed in", but only after the person came back.
    let answer: (r: Response) => void = () => {}
    fetchMock.mockImplementation(() => new Promise<Response>((r) => (answer = r)))
    const got: string[] = []
    const stop = openStream('/x', (d) => got.push(d), { reconnect: true })
    for (let i = 0; i < PROBE_AFTER; i++) {
      at(i).fail()
      if (i < PROBE_AFTER - 1) vi.advanceTimersByTime(backoffDelay(i + 1))
    }
    await vi.advanceTimersByTimeAsync(backoffDelay(PROBE_AFTER))
    expect(fetchMock).toHaveBeenCalledTimes(1)
    // The tab comes back mid-probe: one source opens now…
    win.fire('online')
    expect(FakeES.all).toHaveLength(PROBE_AFTER + 1)
    // …and the probe's late "yes" does not open another.
    answer(new Response(JSON.stringify({ user: { id: 'u' } }), { status: 200, headers: { 'content-type': 'application/json' } }))
    await vi.advanceTimersByTimeAsync(0)
    expect(FakeES.all).toHaveLength(PROBE_AFTER + 1)
    at(PROBE_AFTER).onmessage?.({ data: 'once' })
    expect(got).toEqual(['once'])
    stop()
    expect(at(PROBE_AFTER).closed).toBe(true)
  })

  it('a probe that fails (api down) keeps reconnecting', async () => {
    fetchMock.mockRejectedValue(new Error('502'))
    const stop = openStream('/x', () => {}, { reconnect: true })
    for (let i = 0; i < PROBE_AFTER; i++) {
      at(i).fail()
      await vi.advanceTimersByTimeAsync(backoffDelay(i + 1))
    }
    expect(FakeES.all).toHaveLength(PROBE_AFTER + 1)
    stop()
  })

  // ── Wave 2: a URL read at every connect, the state, the hidden pause ─────

  it('reads a URL getter at every (re)connect — a reconnect carries a fresh cursor', () => {
    let since = 3
    const stop = openStream(() => `/api/me/events?since=${since}`, () => {}, { reconnect: true })
    expect(at(0).url).toBe('/api/me/events?since=3')
    since = 9
    at(0).fail()
    vi.advanceTimersByTime(1000)
    expect(at(1).url).toBe('/api/me/events?since=9')
    stop()
  })

  it('reports its state: connecting → open → reconnecting → open → closed', () => {
    const states: StreamState[] = []
    const stop = openStream('/x', () => {}, { reconnect: true, onState: (s) => states.push(s) })
    at(0).open()
    at(0).fail()
    vi.advanceTimersByTime(1000)
    at(1).open()
    stop()
    expect(states).toEqual(['connecting', 'open', 'reconnecting', 'open', 'closed'])
  })

  it('pauseWhenHidden: closes 30 s after the tab hides, and a quick look back cancels it', () => {
    const stop = openStream('/b', () => {}, { reconnect: true, pauseWhenHidden: true })
    at(0).open()
    doc.visibilityState = 'hidden'
    doc.fire('visibilitychange')
    vi.advanceTimersByTime(HIDDEN_PAUSE_MS - 1)
    expect(at(0).closed).toBe(false)
    doc.visibilityState = 'visible'
    doc.fire('visibilitychange')
    vi.advanceTimersByTime(HIDDEN_PAUSE_MS * 2)
    expect(at(0).closed).toBe(false)
    expect(FakeES.all).toHaveLength(1)
    doc.visibilityState = 'hidden'
    doc.fire('visibilitychange')
    vi.advanceTimersByTime(HIDDEN_PAUSE_MS)
    expect(at(0).closed).toBe(true)
    // Paused is not failing: no reconnect while hidden, however long.
    vi.advanceTimersByTime(10 * 60_000)
    expect(FakeES.all).toHaveLength(1)
    stop()
  })

  it('on show: onResume runs first, then the stream reopens, and onOpen says resumed', async () => {
    const order: string[] = []
    const stop = openStream('/b', () => {}, {
      reconnect: true,
      pauseWhenHidden: 1000,
      onResume: () => {
        order.push('resume')
        return Promise.resolve().then(() => order.push('refetched'))
      },
      onOpen: (i) => order.push(`open first=${i.first} resumed=${!!i.resumed}`),
    })
    at(0).open()
    doc.visibilityState = 'hidden'
    doc.fire('visibilitychange')
    vi.advanceTimersByTime(1000)
    expect(at(0).closed).toBe(true)
    doc.visibilityState = 'visible'
    doc.fire('visibilitychange')
    expect(FakeES.all).toHaveLength(1) // not before the refetch settles
    await vi.advanceTimersByTimeAsync(0)
    expect(FakeES.all).toHaveLength(2)
    at(1).open()
    expect(order).toEqual(['open first=true resumed=false', 'resume', 'refetched', 'open first=false resumed=true'])
    stop()
  })

  it('a stream opened in a tab that is already hidden starts its clock at once', () => {
    doc.visibilityState = 'hidden'
    const stop = openStream('/b', () => {}, { pauseWhenHidden: 2000 })
    vi.advanceTimersByTime(2000)
    expect(at(0).closed).toBe(true)
    stop()
  })

  it('stop() while paused leaves nothing behind', () => {
    const stop = openStream('/b', () => {}, { reconnect: true, pauseWhenHidden: 1000 })
    doc.visibilityState = 'hidden'
    doc.fire('visibilitychange')
    vi.advanceTimersByTime(1000)
    stop()
    doc.visibilityState = 'visible'
    doc.fire('visibilitychange')
    expect(FakeES.all).toHaveLength(1)
    expect(doc.count('visibilitychange')).toBe(0)
  })

  it('the wait is 5 s on an HTTP/1.x page (six sockets per host), 30 s at an edge', () => {
    expect(hiddenPauseMs('http/1.1')).toBe(HIDDEN_PAUSE_H1_MS)
    expect(hiddenPauseMs('h2')).toBe(HIDDEN_PAUSE_MS)
    expect(hiddenPauseMs('h3')).toBe(HIDDEN_PAUSE_MS)
    expect(hiddenPauseMs('')).toBe(HIDDEN_PAUSE_MS)
  })
})

// One `/api/me/events` per browser: the leader tab owns the stream and
// rebroadcasts it; the others listen. Driven over the real leader
// (notify-leader.ts) on a fake browser: a lock manager that queues per name
// and a BroadcastChannel bus that delivers to every OTHER channel.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { FOLLOWER_WAIT_MS, startHub, type HubStreamCallbacks } from './notify-hub'
import { startNotifyLeader, type ChannelLike, type LockManagerLike } from './notify-leader'

function browser() {
  const queue: Array<{ cb: (l: unknown) => Promise<unknown> }> = []
  let held = false
  const pump = () => {
    if (held || queue.length === 0) return
    const next = queue.shift()!
    held = true
    void next.cb({}).finally(() => {
      held = false
      pump()
    })
  }
  const locks: LockManagerLike = {
    request: (_name, cb) =>
      new Promise((resolve) => {
        queue.push({
          cb: (l) => {
            const p = cb(l)
            void p.then(resolve)
            return p
          },
        })
        pump()
      }),
  }
  const channels = new Set<FakeChannel>()
  class FakeChannel implements ChannelLike {
    listeners = new Set<(e: { data: unknown }) => void>()
    open = true
    constructor() {
      channels.add(this)
    }
    postMessage(m: unknown) {
      if (!this.open) throw new Error('closed')
      const data = structuredClone(m)
      for (const c of channels) if (c !== this && c.open) queueMicrotask(() => c.listeners.forEach((l) => l({ data })))
    }
    addEventListener(_t: 'message', l: (e: { data: unknown }) => void) {
      this.listeners.add(l)
    }
    removeEventListener(_t: 'message', l: (e: { data: unknown }) => void) {
      this.listeners.delete(l)
    }
    close() {
      this.open = false
      channels.delete(this)
    }
  }
  return { locks, channel: () => new FakeChannel() }
}

const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}

/** A stand-in for the server: every stream any tab opens, and a way to
 *  push a frame down all of them. */
function server() {
  const streams: Array<{ tab: string; onFrame: (d: string) => void; cb: HubStreamCallbacks; open: boolean }> = []
  return {
    streams,
    live: () => streams.filter((s) => s.open),
    opener: (tab: string) => (onFrame: (d: string) => void, cb: HubStreamCallbacks) => {
      const s = { tab, onFrame, cb, open: true }
      streams.push(s)
      cb.onState('connecting')
      queueMicrotask(() => {
        if (!s.open) return
        cb.onState('open')
        cb.onOpen({ first: true })
      })
      return () => {
        s.open = false
      }
    },
    send: (d: string) => {
      for (const s of streams) if (s.open) s.onFrame(d)
    },
  }
}

function tab(b: ReturnType<typeof browser>, srv: ReturnType<typeof server>, id: string, opts: { hubless?: boolean } = {}) {
  const leader = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: false, focused: false }), tabId: id, hub: !opts.hubless })
  const frames: string[] = []
  const opens: boolean[] = []
  if (opts.hubless) return { id, leader, frames, opens, hub: null, close: () => leader.close() }
  const hub = startHub({ leader, open: srv.opener(id) })
  hub.onFrame((d) => frames.push(d))
  hub.onOpen(({ first }) => opens.push(first))
  return {
    id,
    leader,
    hub,
    frames,
    opens,
    close: () => {
      hub.close()
      leader.close()
    },
  }
}

describe('startHub — one stream per browser', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('5 tabs → 1 stream, and every frame reaches all 5', async () => {
    const b = browser()
    const srv = server()
    const tabs = ['t1', 't2', 't3', 't4', 't5'].map((id) => tab(b, srv, id))
    await flush()
    expect(srv.live().map((s) => s.tab)).toEqual(['t1'])
    expect(tabs.map((t) => t.hub?.role())).toEqual(['leader', 'follower', 'follower', 'follower', 'follower'])
    srv.send('{"type":"notification","notificationId":"n1","seq":1}')
    await flush()
    expect(tabs.map((t) => t.frames.length)).toEqual([1, 1, 1, 1, 1])
    // Each follower had its "first open" from the hub, exactly once.
    expect(tabs.map((t) => t.opens)).toEqual([[true], [true], [true], [true], [true]])
    vi.advanceTimersByTime(FOLLOWER_WAIT_MS * 2)
    await flush()
    expect(srv.live()).toHaveLength(1) // no follower went solo
    for (const t of tabs) t.close()
  })

  it('a follower that joins an open hub hears it on hello', async () => {
    const b = browser()
    const srv = server()
    const t1 = tab(b, srv, 't1')
    await flush()
    const late = tab(b, srv, 'late')
    await flush()
    expect(late.opens).toEqual([true])
    expect(late.hub?.state()).toBe('open')
    t1.close()
    late.close()
  })

  it('the leader closes → the next tab opens its own stream at once, and every tab refetches', async () => {
    const b = browser()
    const srv = server()
    const [t1, t2, t3] = ['t1', 't2', 't3'].map((id) => tab(b, srv, id))
    await flush()
    t1!.close()
    await flush()
    expect(srv.live().map((s) => s.tab)).toEqual(['t2'])
    expect(t2!.hub?.role()).toBe('leader')
    // A reopen for both survivors (first: false): refetch what the handover dropped.
    expect(t2!.opens).toEqual([true, false])
    expect(t3!.opens).toEqual([true, false])
    srv.send('{"type":"notification","notificationId":"n2"}')
    await flush()
    expect([t2!.frames.length, t3!.frames.length]).toEqual([1, 1])
    t2!.close()
    t3!.close()
  })

  it('nobody rebroadcasts (an older build holds the lock): a follower opens its own after the wait', async () => {
    const b = browser()
    const srv = server()
    const old = tab(b, srv, 'old', { hubless: true })
    await flush()
    const t2 = tab(b, srv, 't2')
    await flush()
    expect(srv.live()).toHaveLength(0)
    vi.advanceTimersByTime(FOLLOWER_WAIT_MS)
    await flush()
    expect(t2.hub?.role()).toBe('solo')
    expect(srv.live().map((s) => s.tab)).toEqual(['t2'])
    // The old tab goes; t2 takes the lock and its solo stream becomes the hub's.
    old.close()
    await flush()
    expect(t2.hub?.role()).toBe('leader')
    expect(srv.live().map((s) => s.tab)).toEqual(['t2'])
    t2.close()
  })

  it('a solo tab rejoins a hub that speaks up, and refetches', async () => {
    const b = browser()
    const srv = server()
    const old = tab(b, srv, 'old', { hubless: true })
    await flush()
    const solo = tab(b, srv, 'solo')
    const next = tab(b, srv, 'next')
    vi.advanceTimersByTime(FOLLOWER_WAIT_MS)
    await flush()
    expect([solo.hub?.role(), next.hub?.role()]).toEqual(['solo', 'solo'])
    old.close() // the lock goes to `solo` (first in line)
    await flush()
    expect(solo.hub?.role()).toBe('leader')
    // `next` heard the new leader's state and dropped its own stream.
    expect(next.hub?.role()).toBe('follower')
    expect(srv.live().map((s) => s.tab)).toEqual(['solo'])
    expect(next.opens.at(-1)).toBe(false)
    solo.close()
    next.close()
  })

  it('without locks or a channel every tab opens its own (wave 1)', async () => {
    const srv = server()
    const mk = (id: string) => {
      const leader = startNotifyLeader({ locks: null, channel: null, self: () => ({ visible: true, focused: true }), tabId: id })
      return startHub({ leader, open: srv.opener(id) })
    }
    const hubs = [mk('a'), mk('b')]
    await flush()
    expect(hubs.map((h) => h.role())).toEqual(['solo', 'solo'])
    expect(srv.live()).toHaveLength(2)
    for (const h of hubs) h.close()
  })

  it('resumed() after a freeze is a reopen for that tab only', async () => {
    const b = browser()
    const srv = server()
    const [t1, t2] = ['t1', 't2'].map((id) => tab(b, srv, id))
    await flush()
    t2!.hub?.resumed()
    expect(t2!.opens).toEqual([true, false])
    expect(t1!.opens).toEqual([true])
    t1!.close()
    t2!.close()
  })

  it('the leader\'s state reaches the followers (the status panel)', async () => {
    const b = browser()
    const srv = server()
    const [t1, t2] = ['t1', 't2'].map((id) => tab(b, srv, id))
    await flush()
    const s = srv.live()[0]!
    s.cb.onState('reconnecting')
    await flush()
    expect(t2!.hub?.state()).toBe('reconnecting')
    t1!.close()
    t2!.close()
  })
})

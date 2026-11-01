import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { LEADER_LOCK, startNotifyLeader, type ChannelLike, type LockManagerLike } from './notify-leader'

/** One browser: a lock manager that queues requests per name, and a
 *  BroadcastChannel hub that delivers to every OTHER channel. */
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
    request: (name, cb) => {
      expect(name).toBe(LEADER_LOCK)
      return new Promise((resolve) => {
        queue.push({
          cb: (l) => {
            const p = cb(l)
            void p.then(resolve)
            return p
          },
        })
        pump()
      })
    },
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
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

describe('startNotifyLeader', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('elects exactly one leader, and hands the lock on when it closes', async () => {
    const b = browser()
    const self = () => ({ visible: false, focused: false })
    const t1 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 't1' })
    const t2 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 't2' })
    const t3 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 't3' })
    await flush()
    expect([t1, t2, t3].filter((t) => t.isLeader()).map((t) => t.tabId)).toEqual(['t1'])
    t1.close()
    await flush()
    expect([t2, t3].filter((t) => t.isLeader()).map((t) => t.tabId)).toEqual(['t2'])
    t2.close()
    t3.close()
  })

  it('learns the other tabs at once, and forgets one that says goodbye', async () => {
    const b = browser()
    const t1 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: true, focused: true }), tabId: 't1' })
    const t2 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: false, focused: false }), tabId: 't2' })
    await flush()
    expect(t1.others().map((t) => t.tabId)).toEqual(['t2'])
    expect(t2.others()).toMatchObject([{ tabId: 't1', visible: true, focused: true }])
    t1.close()
    await flush()
    expect(t2.others()).toEqual([])
    t2.close()
  })

  it('re-announces on a heartbeat and on demand', async () => {
    const b = browser()
    let visible = false
    const t1 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible, focused: visible }), tabId: 't1', heartbeatMs: 1000 })
    const t2 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: false, focused: false }), tabId: 't2' })
    await flush()
    visible = true
    t1.announce()
    await flush()
    expect(t2.others()[0]).toMatchObject({ visible: true, focused: true })
    visible = false
    vi.advanceTimersByTime(1000)
    await flush()
    expect(t2.others()[0]).toMatchObject({ visible: false })
    t1.close()
    t2.close()
  })

  it('a claim reaches the leader inside the window; no claim times out', async () => {
    const b = browser()
    const self = () => ({ visible: false, focused: false })
    const leader = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 'lead' })
    const toaster = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 'toast' })
    await flush()
    const claimed = leader.waitForClaim('n1', 'v1', 300)
    toaster.claim('n1', 'v1')
    await flush()
    await expect(claimed).resolves.toBe(true)
    const unclaimed = leader.waitForClaim('n2', 'v1', 300)
    vi.advanceTimersByTime(300)
    await expect(unclaimed).resolves.toBe(false)
    // A claim that already arrived answers at once.
    toaster.claim('n3', 'v1')
    await flush()
    await expect(leader.waitForClaim('n3', 'v1', 300)).resolves.toBe(true)
    leader.close()
    toaster.close()
  })

  it('without a lock manager or a channel, every tab decides alone', async () => {
    const t = startNotifyLeader({ locks: null, channel: null, self: () => ({ visible: false, focused: false }) })
    expect(t.alone).toBe(true)
    expect(t.isLeader()).toBe(true)
    expect(t.others()).toEqual([])
    await expect(t.waitForClaim('n', 'v', 300)).resolves.toBe(false)
    t.close()
  })
})

describe('startNotifyLeader — wave 2', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('carries another protocol\'s messages without reading them, and refuses to send its own types', async () => {
    const b = browser()
    const self = () => ({ visible: false, focused: false })
    const t1 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 't1' })
    const t2 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self, tabId: 't2' })
    const got: unknown[] = []
    t2.onMessage((m) => got.push(m))
    await flush()
    t1.send({ type: 'hub:frame', from: 't1', data: 'x' })
    t1.send({ type: 'bye', tabId: 't1' }) // not an extension: dropped
    await flush()
    expect(got).toEqual([{ type: 'hub:frame', from: 't1', data: 'x' }])
    expect(t2.others().map((t) => t.tabId)).toEqual(['t1'])
    t1.close()
    t2.close()
  })

  it('announces whether it leads, hosts a hub, and its presence client — and re-announces on taking the lock', async () => {
    const b = browser()
    const t1 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: true, focused: true, clientId: 'c1' }), tabId: 't1', hub: true })
    const t2 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: false, focused: false, clientId: 'c2' }), tabId: 't2', hub: true })
    await flush()
    expect(t2.others()[0]).toMatchObject({ tabId: 't1', clientId: 'c1', leader: true, hub: true })
    t1.close()
    await flush()
    expect(t2.isLeader()).toBe(true)
    t2.close()
  })

  it('reads the shared input clock when given one', async () => {
    const b = browser()
    const t1 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: true, focused: true }), tabId: 't1', lastInputAt: () => 1234 })
    const t2 = startNotifyLeader({ locks: b.locks, channel: b.channel(), self: () => ({ visible: false, focused: false }), tabId: 't2' })
    await flush()
    expect(t1.lastInputAt()).toBe(1234)
    expect(t2.others()[0]?.lastInputAt).toBe(1234)
    t1.close()
    t2.close()
  })
})


import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  ACK_MAX,
  CLIENT_KEY,
  clientId,
  clientState,
  createAckBatcher,
  deskClientId,
  newClientId,
  notePresence,
  onClientIdChange,
  onPresenceView,
  presenceView,
  presenceWinner,
  resetClientIdForTests,
  remintClientId,
  resetPresenceForTests,
  startHeartbeat,
  type ClientSignals,
} from './notify-presence'
import type { PresenceBody } from './notify-wire'

const NOW = 1_800_000_000_000
const sig = (over: Partial<ClientSignals> = {}): ClientSignals => ({
  visible: true,
  focused: true,
  lastInputAt: NOW - 1_000,
  now: NOW,
  kind: 'web',
  ...over,
})

// The I-17 table (O-13): looking = shown + focused + input ≤ 60 s; active =
// shown + input ≤ 120 s (the shell: OS input ≤ 120 s); idle / hidden else.
describe('clientState', () => {
  const cases: Array<[string, Partial<ClientSignals>, string]> = [
    ['shown, focused, touched 1 s ago', {}, 'looking'],
    ['shown, focused, touched exactly 60 s ago', { lastInputAt: NOW - 60_000 }, 'looking'],
    ['shown, focused, touched 61 s ago', { lastInputAt: NOW - 61_000 }, 'active'],
    ['shown, unfocused (a second monitor), touched 30 s ago', { focused: false, lastInputAt: NOW - 30_000 }, 'active'],
    ['shown, touched 121 s ago', { lastInputAt: NOW - 121_000 }, 'idle'],
    ['shown, never touched', { lastInputAt: 0 }, 'idle'],
    ['hidden, touched 1 s ago', { visible: false, focused: false }, 'hidden'],
    ['hidden but focus reported (a lying engine)', { visible: false, focused: true }, 'hidden'],
    ['desktop, window hidden, OS input 4 s ago', { kind: 'desktop', visible: false, focused: false, osIdleMs: 4_000 }, 'active'],
    ['desktop, focused, page idle but OS input 10 s ago', { kind: 'desktop', lastInputAt: 0, osIdleMs: 10_000 }, 'looking'],
    ['desktop, OS idle 5 minutes, window hidden', { kind: 'desktop', visible: false, focused: false, osIdleMs: 300_000 }, 'hidden'],
    ['desktop, OS idle 5 minutes, window shown', { kind: 'desktop', lastInputAt: 0, osIdleMs: 300_000 }, 'idle'],
    ['desktop with no OS answer: page signals only', { kind: 'desktop', osIdleMs: null, lastInputAt: NOW - 90_000 }, 'active'],
    ['a web tab ignores an osIdleMs it was handed', { osIdleMs: 1, visible: false, focused: false }, 'hidden'],
  ]
  for (const [name, over, want] of cases) it(`${name} → ${want}`, () => expect(clientState(sig(over))).toBe(want))
})

describe('clientId', () => {
  beforeEach(() => resetClientIdForTests())
  afterEach(() => resetClientIdForTests())

  it('is in the server alphabet, 8–64 chars', () => {
    for (let i = 0; i < 50; i++) expect(newClientId()).toMatch(/^[A-Za-z0-9_-]{8,64}$/)
  })

  it('is kept per tab (sessionStorage) and reused across a reload', () => {
    const m = new Map<string, string>()
    const store = { get: (k: string) => m.get(k) ?? null, set: (k: string, v: string) => void m.set(k, v) }
    const a = clientId(store)
    expect(m.get(CLIENT_KEY)).toBe(a)
    resetClientIdForTests() // a reload: the module starts again, storage stays
    expect(clientId(store)).toBe(a)
  })

  it('replaces a stored id the server would refuse, and survives storage that throws', () => {
    const m = new Map<string, string>([[CLIENT_KEY, 'bad id!']])
    const store = { get: (k: string) => m.get(k) ?? null, set: (k: string, v: string) => void m.set(k, v) }
    expect(clientId(store)).toMatch(/^[A-Za-z0-9_-]{8,64}$/)
    resetClientIdForTests()
    const throws = {
      get: () => {
        throw new Error('denied')
      },
      set: () => {
        throw new Error('denied')
      },
    }
    const id = clientId(throws)
    expect(id).toMatch(/^[A-Za-z0-9_-]{8,64}$/)
    expect(clientId(throws)).toBe(id)
  })
})

describe('remintClientId — a duplicated tab takes a fresh id', () => {
  beforeEach(() => resetClientIdForTests())
  afterEach(() => resetClientIdForTests())

  it('stores a new valid id, answers it from then on, and tells the listeners', () => {
    const m = new Map<string, string>()
    const store = { get: (k: string) => m.get(k) ?? null, set: (k: string, v: string) => void m.set(k, v) }
    const before = clientId(store)
    const heard: string[] = []
    const off = onClientIdChange((id) => heard.push(id))
    const after = remintClientId(store)
    expect(after).not.toBe(before)
    expect(after).toMatch(/^[A-Za-z0-9_-]{8,64}$/)
    expect(m.get(CLIENT_KEY)).toBe(after)
    expect(clientId(store)).toBe(after)
    expect(heard).toEqual([after])
    off()
    remintClientId(store)
    expect(heard).toEqual([after])
  })
})

describe('startHeartbeat — the id is read at each beat', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('a re-minted id beats at once when forced, past the 5 s gap', async () => {
    let t = NOW
    let id = 'tab-old-0001'
    const sent: PresenceBody[] = []
    const hb = startHeartbeat({
      clientId: () => id,
      kind: 'web',
      read: () => ({ visible: true, focused: true, lastInputAt: t }),
      send: async (b) => void sent.push(b),
      now: () => t,
    })
    await vi.advanceTimersByTimeAsync(0)
    expect(sent.map((b) => b.clientId)).toEqual(['tab-old-0001'])
    t += 6_000
    id = 'tab-new-0002'
    hb.poke() // same state, not due: nothing
    await vi.advanceTimersByTimeAsync(0)
    expect(sent).toHaveLength(1)
    hb.poke(true)
    await vi.advanceTimersByTimeAsync(0)
    expect(sent.map((b) => b.clientId)).toEqual(['tab-old-0001', 'tab-new-0002'])
    expect(sent[1]?.state).toBe('looking')
    hb.stop()
  })

  it('a forced beat inside the gap waits for it, never two beats within 5 s', async () => {
    let t = NOW
    const sent: Array<{ at: number }> = []
    const hb = startHeartbeat({
      clientId: 'tab-gap-0001',
      kind: 'web',
      read: () => ({ visible: true, focused: true, lastInputAt: t }),
      send: async () => void sent.push({ at: t }),
      now: () => t,
    })
    await vi.advanceTimersByTimeAsync(0)
    t += 1_000
    hb.poke(true)
    await vi.advanceTimersByTimeAsync(0)
    expect(sent).toHaveLength(1)
    t += 4_000
    await vi.advanceTimersByTimeAsync(4_000)
    expect(sent).toHaveLength(2)
    expect((sent[1]?.at ?? 0) - (sent[0]?.at ?? 0)).toBeGreaterThanOrEqual(5_000)
    hb.stop()
  })
})

describe("deskClientId — the desktop shell's own watcher for this instance", () => {
  const LOCAL = '7d1f0f6e-3a52-4c1e-9d55-0c3f8f2b9a11'
  const reg = [
    { id: '0b9e2c1d-5f4a-4e3b-8c2d-1a0f9e8d7c6b', instanceId: 'beacon-other', url: 'https://other.example' },
    { id: LOCAL, instanceId: 'beacon-here', url: 'https://talaria.example.com' },
  ]
  it("is desk-<the registry entry's local id>, as watch.rs names it — by the beacon's instance id", () => {
    expect(deskClientId(reg, 'beacon-here', 'https://elsewhere.example')).toBe(`desk-${LOCAL}`)
  })
  it('else by origin (trailing slash or not)', () => {
    expect(deskClientId(reg, null, 'https://talaria.example.com')).toBe(`desk-${LOCAL}`)
    expect(deskClientId(reg, '', 'https://talaria.example.com/')).toBe(`desk-${LOCAL}`)
  })
  it('null when the shell does not list this instance, or the id is not one the store takes', () => {
    expect(deskClientId(reg, null, 'https://nope.example')).toBeNull()
    expect(deskClientId([], 'beacon-here', 'https://talaria.example.com')).toBeNull()
    expect(deskClientId([{ id: 'bad id!', instanceId: 'b', url: 'https://x.example' }], 'b', 'https://x.example')).toBeNull()
  })
})

describe('the presence view', () => {
  beforeEach(() => resetPresenceForTests())

  it('the winner is looking, else active, else nobody', () => {
    expect(presenceWinner()).toBeNull()
    notePresence({ looking: null, active: 'b', clients: 2 })
    expect(presenceWinner()).toBe('b')
    notePresence({ looking: 'a', active: 'b' })
    expect(presenceWinner()).toBe('a')
    expect(presenceView().clients).toBe(2) // kept when a heartbeat answer does not say
  })

  it('tells listeners only on a change', () => {
    const seen: Array<string | null> = []
    onPresenceView((v) => seen.push(v.looking))
    notePresence({ looking: 'a', active: null, clients: 1 })
    notePresence({ looking: 'a', active: null, clients: 1 })
    notePresence({ looking: null, active: null, clients: 1 })
    expect(seen).toEqual(['a', null])
  })
})

describe('createAckBatcher (I-18)', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())
  const id = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`

  it('batches for 500 ms, drops what is not a row id, and acks each id once', async () => {
    const post = vi.fn(async (_ids: string[]) => {})
    const b = createAckBatcher(post)
    b.add([id(1), 'talaria-summary', ''])
    b.add([id(2), id(1)])
    expect(post).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(500)
    expect(post).toHaveBeenCalledTimes(1)
    expect(post.mock.calls[0]?.[0]).toEqual([id(1), id(2)])
    b.add([id(1)])
    await vi.advanceTimersByTimeAsync(1000)
    expect(post).toHaveBeenCalledTimes(1)
  })

  it('splits at the route cap and survives a failed post', async () => {
    const post = vi.fn(async (_ids: string[]) => {
      throw new Error('500')
    })
    const b = createAckBatcher(post)
    b.add(Array.from({ length: ACK_MAX + 5 }, (_, i) => id(i)))
    await b.flush()
    expect(post.mock.calls.map((c) => c[0].length)).toEqual([ACK_MAX, 5])
  })
})

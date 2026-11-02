import { describe, expect, it } from 'vitest'
import {
  ARRIVALS_KEY,
  MARGIN_MS,
  PersistedArrivals,
  SEEN_MAX,
  computeArrivals,
  present,
  summaryTitle,
  type ArrivalRecord,
  type ArrivalStore,
} from './notify-arrivals'

const T0 = Date.parse('2026-10-31T12:00:00.000Z')
const iso = (offsetMs: number) => new Date(T0 + offsetMs).toISOString()
const row = (id: string, offsetMs: number, readAt: string | null = null) => ({ id, createdAt: iso(offsetMs), readAt })
type Row = ReturnType<typeof row>

const memStore = (initial: string | null = null): ArrivalStore & { value: string | null } => {
  const s = {
    value: initial,
    read: () => s.value,
    write: (v: string) => {
      s.value = v
    },
  }
  return s
}

describe('computeArrivals (the I-3 rule)', () => {
  it('baselines silently with no record — a fresh profile replays nothing', () => {
    const { arrivals, next } = computeArrivals(null, [row('b', 2000), row('a', 1000)], T0)
    expect(arrivals).toEqual([])
    expect(next.hw).toBe(iso(2000))
    expect(Object.keys(next.seen).sort()).toEqual(['a', 'b'])
    expect(next.at).toBe(T0)
  })

  it('an empty baseline surfaces the first row ever', () => {
    const { next } = computeArrivals(null, [], T0)
    expect(next.hw).toBe('')
    const second = computeArrivals(next, [row('first', 5000)], T0 + 5000)
    expect(second.arrivals.map((a) => a.row.id)).toEqual(['first'])
  })

  it('surfaces unseen unread rows oldest first', () => {
    const rec = computeArrivals(null, [row('a', 0)], T0).next
    const { arrivals } = computeArrivals(rec, [row('d', 3000), row('c', 2000), row('b', 1000), row('a', 0)], T0 + 4000)
    expect(arrivals.map((a) => a.row.id)).toEqual(['b', 'c', 'd'])
    expect(arrivals.every((a) => !a.realert)).toBe(true)
  })

  it('surfaces nothing when the same page refetches', () => {
    const rec = computeArrivals(null, [row('b', 1000), row('a', 0)], T0).next
    expect(computeArrivals(rec, [row('b', 1000), row('a', 0)], T0 + 10).arrivals).toEqual([])
  })

  it('never surfaces a row read elsewhere', () => {
    const rec = computeArrivals(null, [row('a', 0)], T0).next
    const { arrivals, next } = computeArrivals(rec, [row('b', 1000, iso(1500)), row('a', 0)], T0 + 2000)
    expect(arrivals).toEqual([])
    // ...but it is recorded, so it never surfaces later either.
    expect(next.seen.b).toBe(iso(1000))
  })

  it('a known id whose createdAt moved later is a re-alert', () => {
    const rec = computeArrivals(null, [row('dm', 0)], T0).next
    const { arrivals, next } = computeArrivals(rec, [row('dm', 5000)], T0 + 5000)
    expect(arrivals).toEqual([{ row: row('dm', 5000), realert: true }])
    expect(next.seen.dm).toBe(iso(5000))
    // The same version again is nothing new.
    expect(computeArrivals(next, [row('dm', 5000)], T0 + 6000).arrivals).toEqual([])
  })

  it('catches a row that committed after a newer one, inside the margin', () => {
    const rec = computeArrivals(null, [row('new', 10_000)], T0).next
    const late = row('late', 10_000 - MARGIN_MS + 1)
    expect(computeArrivals(rec, [row('new', 10_000), late], T0).arrivals.map((a) => a.row.id)).toEqual(['late'])
    // Older than the margin: that is history, not an arrival.
    const old = row('old', 10_000 - MARGIN_MS - 1)
    expect(computeArrivals(rec, [row('new', 10_000), old], T0).arrivals).toEqual([])
  })

  it('keeps the newest SEEN_MAX ids', () => {
    const rows = Array.from({ length: SEEN_MAX + 20 }, (_, i) => row(`r${i}`, i * 10))
    const { next } = computeArrivals(null, rows, T0)
    expect(Object.keys(next.seen)).toHaveLength(SEEN_MAX)
    expect(next.seen.r0).toBeUndefined()
    expect(next.seen[`r${SEEN_MAX + 19}`]).toBeDefined()
  })
})

describe('present', () => {
  it('shows up to three one by one, more as one summary', () => {
    const a = (id: string) => ({ row: { id, title: id }, realert: false })
    expect(present([])).toBeNull()
    expect(present([a('1'), a('2'), a('3')])).toEqual({ kind: 'each', items: [a('1'), a('2'), a('3')] })
    expect(present([a('1'), a('2'), a('3'), a('4'), a('5')])).toEqual({
      kind: 'summary',
      count: 5,
      newest: { id: '5', title: '5' },
    })
    expect(summaryTitle(5, 'Bob mentioned you')).toBe('5 new — Bob mentioned you…')
  })
})

describe('PersistedArrivals (the record outlives the page)', () => {
  it('a reopened page surfaces what landed while it was closed', () => {
    const store = memStore()
    const first = new PersistedArrivals<Row>('ada', store)
    expect(first.arrive([row('a', 0)], T0)).toEqual([])
    // The page closes; two mentions land; a new page opens on the same store.
    const reopened = new PersistedArrivals<Row>('ada', store)
    const got = reopened.arrive([row('c', 2000), row('b', 1000), row('a', 0)], T0 + 3000)
    expect(got.map((a) => a.row.id)).toEqual(['b', 'c'])
  })

  it('keys the record by user: a second account in the same browser baselines on its own', () => {
    const store = memStore()
    new PersistedArrivals<Row>('ada', store).arrive([row('a', 0)], T0)
    const bob = new PersistedArrivals<Row>('bob', store)
    expect(bob.arrive([row('x', 1000)], T0 + 1000)).toEqual([])
    const all = JSON.parse(store.value!) as Record<string, ArrivalRecord>
    expect(Object.keys(all).sort()).toEqual(['ada', 'bob'])
  })

  it('judges against its own view after the first read — another tab surfacing a row does not hide it here', () => {
    const store = memStore()
    const tabA = new PersistedArrivals<Row>('ada', store)
    const tabB = new PersistedArrivals<Row>('ada', store)
    tabA.arrive([row('a', 0)], T0)
    tabB.arrive([row('a', 0)], T0)
    expect(tabA.arrive([row('b', 1000), row('a', 0)], T0 + 1000).map((a) => a.row.id)).toEqual(['b'])
    expect(tabB.arrive([row('b', 1000), row('a', 0)], T0 + 1000).map((a) => a.row.id)).toEqual(['b'])
  })

  it('a new tab does not re-surface what an open tab already surfaced', () => {
    const store = memStore()
    const open = new PersistedArrivals<Row>('ada', store)
    open.arrive([row('a', 0)], T0)
    expect(open.arrive([row('b', 1000), row('a', 0)], T0 + 1000).map((a) => a.row.id)).toEqual(['b'])
    const fresh = new PersistedArrivals<Row>('ada', store)
    expect(fresh.arrive([row('b', 1000), row('a', 0)], T0 + 2000)).toEqual([])
  })

  it('survives a store that holds junk or refuses access', () => {
    const junk = memStore('{not json')
    expect(new PersistedArrivals<Row>('ada', junk).arrive([row('a', 0)], T0)).toEqual([])
    const refusing: ArrivalStore = { read: () => null, write: () => {} }
    const t = new PersistedArrivals<Row>('ada', refusing)
    expect(t.arrive([row('a', 0)], T0)).toEqual([])
    expect(t.arrive([row('b', 1000), row('a', 0)], T0 + 1000).map((a) => a.row.id)).toEqual(['b'])
  })

  it('writes the documented key shape', () => {
    expect(ARRIVALS_KEY).toBe('talaria.notify.arrivals.v1')
    const store = memStore()
    new PersistedArrivals<Row>('ada', store).arrive([row('a', 0)], T0)
    const all = JSON.parse(store.value!) as Record<string, ArrivalRecord>
    expect(all.ada).toEqual({ hw: iso(0), seen: { a: iso(0) }, at: T0 })
  })
})

import { describe, expect, it } from 'vitest'
import {
  ARRIVALS_KEY,
  MARGIN_MS,
  PersistedArrivals,
  SEEN_MAX,
  coalesce,
  computeArrivals,
  frameNeedsRead,
  present,
  readSince,
  rebaseArrivals,
  summaryTitle,
  type ArrivalRecord,
  type ArrivalStore,
  type SincePage,
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

// ── I-20: the record v2 and the seq rule ─────────────────────────────────────

type SRow = { id: string; createdAt: string; readAt: string | null; seq: number | null }
const srow = (id: string, seq: number | null, offsetMs: number, readAt: string | null = null): SRow => ({ id, createdAt: iso(offsetMs), readAt, seq })

describe('computeArrivals — v2 (a record with a seq cursor)', () => {
  const rec = (seq: number, seen: Record<string, string> = {}, hw = iso(0)): ArrivalRecord => ({ hw, seen, at: T0, v: 2, seq })

  it('a row past the cursor is an arrival with no margin at all — even one much older than the high-water', () => {
    // Committed long after it was created (a slow transaction): wave 1's
    // margin would have absorbed it; the cursor does not.
    const { arrivals } = computeArrivals(rec(10, {}, iso(600_000)), [srow('late', 11, 0)], T0 + 600_000)
    expect(arrivals.map((a) => a.row.id)).toEqual(['late'])
  })

  it('at or below the cursor is never an arrival, unseen or not', () => {
    const { arrivals } = computeArrivals(rec(10), [srow('a', 10, 5000), srow('b', 9, 6000)], T0 + 7000)
    expect(arrivals).toEqual([])
  })

  it('a quiet coalesce (new seq, same createdAt, already seen) is not news; a re-alert (createdAt moved) is', () => {
    const r = rec(10, { g: iso(1000) })
    expect(computeArrivals(r, [srow('g', 11, 1000)], T0 + 2000).arrivals).toEqual([])
    const re = computeArrivals(r, [srow('g', 12, 3000)], T0 + 4000).arrivals
    expect(re.map((a) => [a.row.id, a.realert])).toEqual([['g', true]])
  })

  it('read rows past the cursor are recorded, not surfaced', () => {
    const { arrivals, next } = computeArrivals(rec(10), [srow('r', 11, 1000, iso(1500))], T0 + 2000)
    expect(arrivals).toEqual([])
    expect(next.seen.r).toBe(iso(1000))
  })

  it('a legacy row (no seq) keeps wave 1\'s rule under a v2 record', () => {
    const r = rec(10, {}, iso(MARGIN_MS * 3))
    expect(computeArrivals(r, [srow('old', null, 0)], T0).arrivals).toEqual([])
    expect(computeArrivals(r, [srow('fresh', null, MARGIN_MS * 3 + 1)], T0).arrivals.map((a) => a.row.id)).toEqual(['fresh'])
  })

  it('the cursor moves only to `through`, and writes v: 2', () => {
    const a = computeArrivals(rec(10), [srow('x', 11, 1000)], T0)
    expect(a.next.seq).toBe(10)
    const b = computeArrivals(rec(10), [srow('x', 11, 1000)], T0, 14)
    expect(b.next).toMatchObject({ v: 2, seq: 14 })
  })

  it('a wave-1 record (no seq) on its first v2 read: wave 1\'s rule decides, so nothing it already surfaced comes back', () => {
    const v1 = computeArrivals(null, [srow('a', 1, 1000), srow('b', 2, 2000)], T0).next
    delete v1.seq
    delete v1.v
    // The first `?since=0` read returns both again, plus one new row.
    const { arrivals, next } = computeArrivals(v1, [srow('a', 1, 1000), srow('b', 2, 2000), srow('c', 3, 3000)], T0 + 3000, 3)
    expect(arrivals.map((x) => x.row.id)).toEqual(['c'])
    expect(next).toMatchObject({ v: 2, seq: 3 })
  })

  it('no record at all: a silent baseline whose cursor starts at the snapshot', () => {
    const { arrivals, next } = computeArrivals(null, [srow('a', 4, 1000)], T0, 9)
    expect(arrivals).toEqual([])
    expect(next).toMatchObject({ v: 2, seq: 9 })
  })
})

describe('readSince — the ?since pager', () => {
  const pages = (all: SRow[], size: number, snap: number) => async (since: number): Promise<SincePage<SRow>> => {
    const after = all.filter((r) => (r.seq ?? 0) > since).sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))
    return { notifications: after.slice(0, size), more: after.length > size, seq: snap }
  }
  const rows = (n: number, from = 1) => Array.from({ length: n }, (_, i) => srow(`r${from + i}`, from + i, i * 10))

  it('one page: every row, through the snapshot seq', async () => {
    const r = await readSince(pages(rows(60), 200, 60), 0)
    expect(r.rows).toHaveLength(60)
    expect(r).toMatchObject({ through: 60, complete: true })
  })

  it('pages until `more` is false', async () => {
    const r = await readSince(pages(rows(450), 200, 450), 0)
    expect(r.rows.map((x) => x.seq)).toEqual(Array.from({ length: 450 }, (_, i) => i + 1))
    expect(r).toMatchObject({ through: 450, complete: true })
  })

  it('stops after the page budget, complete up to the last row read', async () => {
    const r = await readSince(pages(rows(450), 200, 450), 0, 2)
    expect(r).toMatchObject({ through: 400, complete: false })
  })

  it('a page that says `more` but moves nothing does not loop', async () => {
    const r = await readSince(async () => ({ notifications: [], more: true, seq: 5 }), 3)
    expect(r).toMatchObject({ through: null, complete: false })
  })

  it('the snapshot seq can run ahead of the last row (rows read meanwhile): the cursor takes it', async () => {
    const r = await readSince(async () => ({ notifications: [srow('a', 4, 0)], more: false, seq: 7 }), 3)
    expect(r.through).toBe(7)
  })

  it("the server's seq BELOW the cursor (a restore): `behind`, through the server's seq — never the larger one", async () => {
    // The hub's answer after a restore: no rows past 1004, seq 4.
    const r = await readSince(async () => ({ notifications: [], more: false, seq: 4 }), 1004)
    expect(r).toMatchObject({ rows: [], through: 4, complete: true, behind: true })
    const level = await readSince(async () => ({ notifications: [], more: false, seq: 1004 }), 1004)
    expect(level.behind).toBeUndefined()
    expect(level.through).toBe(1004)
  })
})

describe('the rebase — the server\'s seq went back (a restored database, a reset dev stack)', () => {
  // The browser had read through 1004 at T0 + 10 min; the restore puts the
  // server back at seq 4, and rows filed since take 5, 6, … — all below the
  // old cursor.
  const before = (): ArrivalRecord => ({
    hw: iso(600_000),
    seen: { old1: iso(590_000), old2: iso(600_000) },
    at: T0 + 600_000,
    v: 2,
    seq: 1004,
  })

  it('a row filed since the restore surfaces; nothing older replays; the cursor is the server\'s seq', () => {
    const page = [srow('new5', 5, 660_000), srow('old2', 2, 600_000), srow('old1', 1, 590_000), srow('ancient', 3, 0)]
    const { arrivals, next } = rebaseArrivals(before(), page, T0 + 661_000, 5)
    expect(arrivals.map((a) => a.row.id)).toEqual(['new5'])
    expect(next).toMatchObject({ v: 2, seq: 5, rebased: T0 + 661_000 })
    expect(next.seen.new5).toBe(iso(660_000))
    // …and the cursor rule takes over from the new base: 6 is an arrival.
    expect(computeArrivals(next, [srow('new6', 6, 662_000)], T0 + 663_000, 6).arrivals.map((a) => a.row.id)).toEqual(['new6'])
  })

  it('without the rebase, the same row is never news (the defect)', () => {
    expect(computeArrivals(before(), [srow('new5', 5, 660_000)], T0 + 661_000, 1004).arrivals).toEqual([])
  })

  it('no record at all: the silent baseline, at the server\'s seq', () => {
    const { arrivals, next } = rebaseArrivals(null, [srow('a', 3, 0)], T0, 3)
    expect(arrivals).toEqual([])
    expect(next).toMatchObject({ v: 2, seq: 3 })
  })

  it("persisted: the lower seq is kept — a merge with another tab's pre-restore cursor does not put 1004 back", () => {
    const store = memStore(JSON.stringify({ u1: before() }))
    const a = new PersistedArrivals<SRow>('u1', store)
    const stale = new PersistedArrivals<SRow>('u1', store)
    expect(stale.cursor()).toBe(1004) // primed before the restore
    expect(a.rebase([srow('new5', 5, 660_000)], 5, T0 + 661_000).map((x) => x.row.id)).toEqual(['new5'])
    expect(JSON.parse(store.value ?? '{}').u1).toMatchObject({ seq: 5, rebased: T0 + 661_000 })
    expect(a.persistedCursor()).toBe(5)
    // The stale tab writes its own view (cursor 1004, never rebased): the
    // record rebased last holds the cursor.
    stale.arrive([], T0 + 662_000, 1004)
    expect(JSON.parse(store.value ?? '{}').u1.seq).toBe(5)
    // The rebased tab reads on from its new base: the usual max again.
    a.arrive([srow('new6', 6, 662_000)], T0 + 663_000, 6)
    expect(JSON.parse(store.value ?? '{}').u1.seq).toBe(6)
    // The stale tab's own read then finds the server behind it and rebases
    // too, later: its cursor (the server's, now 7) wins.
    stale.rebase([srow('new7', 7, 664_000)], 7, T0 + 665_000)
    expect(JSON.parse(store.value ?? '{}').u1).toMatchObject({ seq: 7, rebased: T0 + 665_000 })
  })

  it('a junk `rebased` in storage is dropped', () => {
    const store = memStore(JSON.stringify({ u1: { ...before(), rebased: 'x' } }))
    const t = new PersistedArrivals<SRow>('u1', store)
    t.arrive([], T0 + 700_000, 1004)
    expect(JSON.parse(store.value ?? '{}').u1.rebased).toBeUndefined()
  })
})

describe('coalesce', () => {
  it('one run at a time; any number of asks during it is one more run', async () => {
    let runs = 0
    let release: () => void = () => {}
    const go = coalesce(async () => {
      runs++
      if (runs === 1) await new Promise<void>((r) => (release = r))
    })
    const first = go()
    for (let i = 0; i < 60; i++) void go()
    expect(runs).toBe(1)
    release()
    await first
    expect(runs).toBe(2)
  })

  it('a throwing run does not wedge it', async () => {
    let runs = 0
    const go = coalesce(async () => {
      runs++
      throw new Error('500')
    })
    await go()
    await go()
    expect(runs).toBe(2)
  })
})

describe('PersistedArrivals — v2', () => {
  it('the 60-row outage: one catch-up, every id in seen, the cursor at the server seq, nothing twice', () => {
    const store = memStore()
    const t = new PersistedArrivals<SRow>('u1', store)
    t.arrive([], T0, 100) // the baseline, at seq 100
    const burst = Array.from({ length: 60 }, (_, i) => srow(`m${i}`, 101 + i, 1000 + i))
    const got = t.arrive(burst, T0 + 5000, 160)
    expect(got).toHaveLength(60)
    const rec = JSON.parse(store.value ?? '{}').u1
    expect(rec.seq).toBe(160)
    expect(rec.v).toBe(2)
    expect(burst.every((r) => r.id in rec.seen)).toBe(true)
    // The same rows read again (a second catch-up racing the first): nothing.
    expect(t.arrive(burst, T0 + 6000, 160)).toEqual([])
    expect(t.cursor()).toBe(160)
  })

  it('a second tab opening later starts from the persisted cursor', () => {
    const store = memStore()
    const a = new PersistedArrivals<SRow>('u1', store)
    a.arrive([], T0, 5)
    a.arrive([srow('x', 6, 1000)], T0 + 1000, 6)
    const b = new PersistedArrivals<SRow>('u1', store)
    expect(b.hasRecord()).toBe(true)
    expect(b.cursor()).toBe(6)
    expect(b.arrive([srow('x', 6, 1000)], T0 + 2000, 6)).toEqual([])
  })

  it('merging two tabs keeps the furthest cursor', () => {
    const store = memStore()
    const a = new PersistedArrivals<SRow>('u1', store)
    const b = new PersistedArrivals<SRow>('u1', store)
    a.arrive([], T0, 10)
    b.arrive([], T0, 4) // b primed from the store (10) — its own view stays 10
    expect(JSON.parse(store.value ?? '{}').u1.seq).toBe(10)
    expect(b.persistedCursor()).toBe(10)
  })

  it('a junk cursor in storage is no cursor', () => {
    const store = memStore(JSON.stringify({ u1: { hw: iso(0), seen: {}, at: T0, v: 2, seq: 'x' } }))
    const t = new PersistedArrivals<SRow>('u1', store)
    expect(t.hasRecord()).toBe(true)
    expect(t.cursor()).toBeNull()
  })

  it('a wave-1 reader still finds hw/seen/at where it always did', () => {
    const store = memStore()
    const t = new PersistedArrivals<SRow>('u1', store)
    t.arrive([srow('a', 1, 1000)], T0, 1)
    const rec = JSON.parse(store.value ?? '{}').u1
    expect(Object.keys(rec).sort()).toEqual(['at', 'hw', 'seen', 'seq', 'v'])
    expect(typeof rec.hw).toBe('string')
  })
})

describe('frameNeedsRead — a replay of rows already read asks for nothing', () => {
  it('reads a notification past the cursor, skips one at or below it', () => {
    expect(frameNeedsRead({ type: 'notification', seq: 11 }, 10)).toBe(true)
    expect(frameNeedsRead({ type: 'notification', seq: 10 }, 10)).toBe(false)
    expect(frameNeedsRead({ type: 'notification', seq: 3 }, 10)).toBe(false)
  })
  it('reads when it cannot tell: no seq (an older api), no cursor yet', () => {
    expect(frameNeedsRead({ type: 'notification' }, 10)).toBe(true)
    expect(frameNeedsRead({ type: 'notification', seq: 3 }, null)).toBe(true)
  })
  it('a complete resync at the cursor is done; an incomplete one always reads', () => {
    expect(frameNeedsRead({ type: 'resync', seq: 10, complete: true }, 10)).toBe(false)
    expect(frameNeedsRead({ type: 'resync', seq: 12, complete: true }, 10)).toBe(true)
    expect(frameNeedsRead({ type: 'resync', seq: 9, complete: false }, 10)).toBe(true)
  })
  it("a resync BELOW the cursor reads: the server's seq went back (a restore), and the read rebases", () => {
    expect(frameNeedsRead({ type: 'resync', seq: 4, complete: true }, 1004)).toBe(true)
    // A replayed notification frame below the cursor is still nothing.
    expect(frameNeedsRead({ type: 'notification', seq: 4 }, 1004)).toBe(false)
  })
  it('other frames never read', () => {
    expect(frameNeedsRead({ type: 'notifications' }, 10)).toBe(false)
    expect(frameNeedsRead({ type: 'presence' }, null)).toBe(false)
  })
})

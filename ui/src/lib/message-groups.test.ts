import { describe, expect, it } from 'vitest'
import { dayDividerMap } from '@/lib/day-dividers'
import { continuationSet, GROUP_GAP_MS, messageBlocks, type GroupFacts } from '@/lib/message-groups'

type Row = GroupFacts & { id: string }

const T0 = Date.parse('2026-11-04T15:00:00Z')
const min = 60 * 1000
const row = (id: string, sender: string, offsetMin: number | null, extra: Partial<Row> = {}): Row => ({
  id,
  sender,
  at: offsetMin === null ? undefined : new Date(T0 + offsetMin * min).toISOString(),
  ...extra,
})
const noDays = { has: () => false }
// The blocks as id runs, which reads like the transcript.
const runs = (rows: Row[], dayStarts: { has(i: number): boolean } = noDays) =>
  messageBlocks(rows, (r) => r, { dayStarts }).map((b) => rows.slice(b.start, b.end + 1).map((r) => r.id))

describe('messageBlocks', () => {
  it('the threshold is five minutes, as the changelog promises', () => {
    expect(GROUP_GAP_MS).toBe(5 * 60 * 1000)
  })

  it('groups the same sender inside the threshold', () => {
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', 1), row('a3', 'user:a', 4)])).toEqual([['a1', 'a2', 'a3']])
  })

  it('measures the gap between consecutive messages, not from the block start', () => {
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', 4), row('a3', 'user:a', 8)])).toEqual([['a1', 'a2', 'a3']])
  })

  it('breaks on a sender change, and a sender coming back starts a new block', () => {
    expect(runs([row('a1', 'user:a', 0), row('b1', 'user:b', 1), row('a2', 'user:a', 2)])).toEqual([['a1'], ['b1'], ['a2']])
  })

  it('a user and an agent never share a block', () => {
    expect(runs([row('u', 'user:a', 0), row('g', 'agent:m', 0)])).toEqual([['u'], ['g']])
  })

  it('a gap of exactly the threshold still groups; one millisecond more breaks', () => {
    const at = (ms: number) => new Date(T0 + ms).toISOString()
    const exact = [row('a1', 'user:a', 0), { id: 'a2', sender: 'user:a', at: at(GROUP_GAP_MS) }]
    expect(runs(exact)).toEqual([['a1', 'a2']])
    const over = [row('a1', 'user:a', 0), { id: 'a2', sender: 'user:a', at: at(GROUP_GAP_MS + 1) }]
    expect(runs(over)).toEqual([['a1'], ['a2']])
  })

  it('breaks at a day boundary even inside the threshold', () => {
    // 23:58 and 00:01 UTC, three minutes apart, either side of midnight.
    const rows: Row[] = [
      { id: 'late', sender: 'user:a', at: '2026-11-03T23:58:00Z' },
      { id: 'early', sender: 'user:a', at: '2026-11-04T00:01:00Z' },
      { id: 'next', sender: 'user:a', at: '2026-11-04T00:02:00Z' },
    ]
    const days = dayDividerMap(rows, (r) => r.at!, { timeZone: 'UTC', now: Date.parse('2026-11-04T12:00:00Z') })
    expect(runs(rows, days)).toEqual([['late'], ['early', 'next']])
  })

  it('a deleted message leaves the list, and the rows around it regroup', () => {
    const before = [row('a1', 'user:a', 0), row('b1', 'user:b', 1), row('a2', 'user:a', 2)]
    expect(runs(before.filter((r) => r.id !== 'b1'))).toEqual([['a1', 'a2']])
    // Deleting a block's head promotes the next row to carry the header.
    const head = [row('a1', 'user:a', 0), row('a2', 'user:a', 1), row('a3', 'user:a', 2)]
    const blocks = messageBlocks(head.slice(1), (r) => r, { dayStarts: noDays })
    expect(blocks).toEqual([{ start: 0, end: 1, sender: 'user:a' }])
  })

  it('a fresh row starts its own block, and later rows can join it', () => {
    // The mechanism behind a thread's first reply and a scheduled run's turn;
    // the surfaces' own rules are tested with their fact helpers.
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', 1, { fresh: true }), row('a3', 'user:a', 2)])).toEqual([
      ['a1'],
      ['a2', 'a3'],
    ])
  })

  it('a row stamped before the one above it starts a block, however small the gap', () => {
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', -0.1)])).toEqual([['a1'], ['a2']])
    // Clock skew: the client drew U2 on a clock 20 minutes slow, so it reads
    // five minutes BEFORE the server-stamped U1. It must not join U1 only for
    // its header to pop in when the sync swaps in the real (15-minute-later)
    // time — it starts a block both before and after the swap.
    const drawn = [row('u1', 'user:a', 0), row('u2', 'user:a', -5)]
    const synced = [row('u1', 'user:a', 0), row('u2', 'user:a', 15)]
    expect(runs(drawn)).toEqual([['u1'], ['u2']])
    expect(runs(synced)).toEqual(runs(drawn))
    // The same instant is a zero gap, not a negative one: it groups.
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', 0)])).toEqual([['a1', 'a2']])
  })

  it('a row with no time that parses starts a block, and so does the row after it', () => {
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', null), row('a3', 'user:a', 1)])).toEqual([['a1'], ['a2'], ['a3']])
    expect(runs([row('a1', 'user:a', 0), { id: 'bad', sender: 'user:a', at: 'nope' }])).toEqual([['a1'], ['bad']])
  })

  it('accepts epoch-ms and Date instants', () => {
    const rows = [
      { id: 'a', sender: 'user:a', at: T0 },
      { id: 'b', sender: 'user:a', at: new Date(T0 + min) },
    ]
    expect(messageBlocks(rows, (r) => r, { dayStarts: noDays })).toEqual([{ start: 0, end: 1, sender: 'user:a' }])
  })

  it('an empty list has no blocks', () => {
    expect(messageBlocks([], () => ({ sender: 'x', at: T0 }), { dayStarts: noDays })).toEqual([])
  })
})

describe('continuationSet', () => {
  it('every row of a block but its first', () => {
    const rows = [row('a1', 'user:a', 0), row('a2', 'user:a', 1), row('b1', 'user:b', 2), row('b2', 'user:b', 3), row('b3', 'user:b', 4)]
    const set = continuationSet(messageBlocks(rows, (r) => r, { dayStarts: noDays }))
    expect([...set].sort()).toEqual([1, 3, 4])
  })
})

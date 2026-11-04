import { describe, expect, it } from 'vitest'
import { dayDividerMap } from '@/lib/day-dividers'
import { continuationSet, GROUP_GAP_MS, messageBlocks, type GroupFacts } from '@/lib/message-groups'

type Row = GroupFacts & { id: string }

const T0 = Date.parse('2026-11-04T15:00:00Z')
const min = 60 * 1000
const row = (id: string, sender: string | null, offsetMin: number | null, extra: Partial<Row> = {}): Row => ({
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
  it('the threshold is in the 5-10 minute range', () => {
    expect(GROUP_GAP_MS).toBeGreaterThanOrEqual(5 * min)
    expect(GROUP_GAP_MS).toBeLessThanOrEqual(10 * min)
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

  it('threaded replies: a fresh row starts its own block, and later rows can join it', () => {
    // The thread panel: the root, then the replies under the rule. The first
    // reply is fresh, so the root stands alone even from its own author.
    const thread = [row('root', 'user:a', 0), row('r1', 'user:a', 1), row('r2', 'user:a', 2), row('r3', 'user:b', 3)]
    const blocks = messageBlocks(thread, (r, i) => ({ ...r, fresh: i === 1 }), { dayStarts: noDays })
    expect(blocks.map((b) => [b.start, b.end])).toEqual([[0, 0], [1, 2], [3, 3]])
    // The channel feed: a reply surfaced from a thread carries its own header.
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', 1, { fresh: true })])).toEqual([['a1'], ['a2']])
  })

  it('a system notice stands alone and breaks the run around it', () => {
    expect(runs([row('a1', 'user:a', 0), row('sys', null, 1), row('a2', 'user:a', 2)])).toEqual([['a1'], ['sys'], ['a2']])
    expect(runs([row('s1', null, 0), row('s2', null, 0)])).toEqual([['s1'], ['s2']])
  })

  it('a row with no time that parses starts a block, and so does the row after it', () => {
    expect(runs([row('a1', 'user:a', 0), row('a2', 'user:a', null), row('a3', 'user:a', 1)])).toEqual([['a1'], ['a2'], ['a3']])
    expect(runs([row('a1', 'user:a', 0), { id: 'bad', sender: 'user:a', at: 'nope' }])).toEqual([['a1'], ['bad']])
  })

  it('a streaming last message groups as it will once settled, so a token never moves a header', () => {
    // The facts carry no status or content: the same sender and time give the
    // same blocks while it streams, after it settles, and as content arrives.
    type Msg = { id: string; author: string; createdAt: string; status: 'streaming' | 'complete'; content: string }
    const at = (m: number) => new Date(T0 + m * min).toISOString()
    const base: Msg[] = [
      { id: 'u', author: 'user:a', createdAt: at(0), status: 'complete', content: 'hi' },
      { id: 'g1', author: 'agent:m', createdAt: at(0.1), status: 'complete', content: 'on it' },
    ]
    const facts = (m: Msg) => ({ sender: m.author, at: m.createdAt })
    const blocksOf = (last: Msg) => messageBlocks([...base, last], facts, { dayStarts: noDays })
    const streaming = { id: 'g2', author: 'agent:m', createdAt: at(0.5), status: 'streaming' as const, content: '' }
    const grown = { ...streaming, content: 'Here is the first part of the answer' }
    const settled = { ...grown, status: 'complete' as const }
    expect(blocksOf(streaming)).toEqual([{ start: 0, end: 0, sender: 'user:a' }, { start: 1, end: 2, sender: 'agent:m' }])
    expect(blocksOf(grown)).toEqual(blocksOf(streaming))
    expect(blocksOf(settled)).toEqual(blocksOf(streaming))
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

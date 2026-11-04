import { describe, expect, it } from 'vitest'
import {
  clock,
  fromLedger,
  layout,
  livePath,
  path,
  summarize,
  toolKind,
  type Checkpoint,
  type LedgerRow,
  type Placed,
} from './agent-checkpoints'

const cp = (id: string, parent?: string, extra: Partial<Checkpoint> = {}): Checkpoint => ({
  id,
  title: id,
  tool: 'read',
  parent,
  status: 'done',
  at: 0,
  ...extra,
})

const line = (...ids: string[]) => ids.map((id, i) => cp(id, ids[i - 1]))
const placedOf = (placed: Placed[], id: string) => placed.find((p) => p.cp.id === id)!
const order = (placed: Placed[]) => placed.map((p) => p.cp.id)

const row = (seq: number, extra: Partial<LedgerRow> = {}): LedgerRow => ({
  seq,
  tool: null,
  title: `turn ${seq} recorded`,
  status: 'send',
  createdAt: new Date(1_800_000_000_000 + seq * 1000).toISOString(),
  ...extra,
})

describe('layout', () => {
  it('draws no rows and one lane for an empty run', () => {
    expect(summarize(layout([]))).toEqual({ checkpoints: 0, attempts: 1, lanes: 1 })
  })

  it('keeps one attempt on one straight lane', () => {
    const placed = layout(line('a', 'b', 'c'))
    expect(placed.map((p) => p.lane)).toEqual([0, 0, 0])
    expect(placed.map((p) => p.attempt)).toEqual([undefined, undefined, undefined])
    expect(path(placedOf(placed, 'c') as Placed & { from: number; fromLane: number })).not.toMatch(/C/)
  })

  it('draws a rewind out of its target on a lane of its own', () => {
    const placed = layout([...line('a', 'b', 'c'), cp('d', 'a', { attempt: 2 })])
    const fork = placedOf(placed, 'd')
    expect(fork.from).toBe(0)
    expect(fork.lane).not.toBe(fork.fromLane)
    expect(path(fork as Placed & { from: number; fromLane: number })).toMatch(/C/)
  })

  it('gives a rewind to a line tip its own lane when it says it forks', () => {
    const placed = layout([...line('a', 'b'), cp('c', 'b', { fork: true, attempt: 2 })])
    expect(placedOf(placed, 'c').lane).toBe(1)
  })

  it('gives a second child of one row its own lane', () => {
    const placed = layout([...line('a', 'b'), cp('c', 'a'), cp('d', 'a')])
    expect(new Set(placed.slice(1).map((p) => p.lane)).size).toBe(3)
  })

  it('reuses a lane once nothing below can collide with it', () => {
    const placed = layout([...line('a', 'b'), cp('c', 'b', { fork: true }), cp('d', undefined)])
    expect(placedOf(placed, 'd').lane).toBe(0)
  })
})

describe('a late parent', () => {
  it('moves the child under its parent instead of starting a new line', () => {
    const placed = layout([cp('a'), cp('c', 'b'), cp('b', 'a')])
    expect(order(placed)).toEqual(['a', 'b', 'c'])
    expect(placed.map((p) => p.lane)).toEqual([0, 0, 0])
    expect(placedOf(placed, 'c').from).toBe(1)
  })

  it('keeps a whole chain that arrived in reverse', () => {
    expect(order(layout([cp('d', 'c'), cp('c', 'b'), cp('b', 'a'), cp('a')]))).toEqual(['a', 'b', 'c', 'd'])
  })

  it('still draws a row whose parent never came', () => {
    const placed = layout([cp('a'), cp('b', 'gone')])
    expect(order(placed)).toEqual(['a', 'b'])
    expect(placedOf(placed, 'b').from).toBeUndefined()
  })
})

describe('attempts', () => {
  it('shows the number the server gave, not a count of forks', () => {
    const placed = layout([cp('a', undefined, { attempt: 2 }), cp('b', 'a', { attempt: 2 }), cp('c', 'a', { attempt: 7 })])
    expect(placedOf(placed, 'c').attempt).toBe(7)
  })

  it('invents no number when the server sent none', () => {
    expect(placedOf(layout([...line('a', 'b'), cp('c', 'a')]), 'c').attempt).toBeUndefined()
  })

  it('counts the attempts the server named', () => {
    const placed = layout([cp('a', undefined, { attempt: 1 }), cp('b', 'a', { attempt: 2 }), cp('c', 'a', { attempt: 3 })])
    expect(summarize(placed).attempts).toBe(3)
  })

  it('chips only the row that opens an attempt', () => {
    const placed = layout([cp('a', undefined, { attempt: 1 }), cp('b', 'a', { attempt: 2, fork: true }), cp('c', 'b', { attempt: 2 })])
    expect(placed.map((p) => p.attempt)).toEqual([undefined, 2, undefined])
  })
})

describe('livePath', () => {
  it('walks from the newest row back to the first', () => {
    const placed = layout([...line('a', 'b', 'c'), cp('d', 'a', { attempt: 2 })])
    expect([...livePath(placed)].sort()).toEqual([0, 3])
  })

  it('is everything on a single attempt', () => {
    expect(livePath(layout(line('a', 'b', 'c'))).size).toBe(3)
  })

  it('is empty for an empty run', () => {
    expect(livePath([]).size).toBe(0)
  })
})

describe('toolKind', () => {
  it.each([
    ['terminal', 'shell'],
    ['read_file', 'read'],
    ['write_file', 'edit'],
    ['patch', 'edit'],
    ['search_files', 'search'],
    ['browser_navigate', 'browser'],
    ['mcp__workbench__start_job', 'other'],
    ['mcp__workbench__run_tests', 'test'],
    ['mcp__talaria__get_ticket', 'other'],
    ['mcp__github_tools__create_pr', 'git'],
  ])('reads %s as %s', (name, kind) => {
    expect(toolKind(name)).toBe(kind)
  })

  it('falls back for a tool nobody has met', () => {
    expect(toolKind('quantum_flux_capacitor')).toBe('other')
  })

  it('does not match inside a word', () => {
    expect(toolKind('thread_summary')).toBe('other')
  })

  it('reads a row with no tool as a turn', () => {
    expect(toolKind(null)).toBe('turn')
  })
})

describe('fromLedger', () => {
  it('chains each row to the one before it, timed from the first', () => {
    const cps = fromLedger([row(1), row(2), row(4)])
    expect(cps.map((c) => c.parent)).toEqual([undefined, '1', '2'])
    expect(cps.map((c) => c.at)).toEqual([0, 1000, 3000])
  })

  it('reads as a sentence, and names a row that has no title', () => {
    const cps = fromLedger([
      row(1),
      row(2, { title: null, tool: 'mcp__workbench__start_job' }),
      row(3, { title: null }),
    ])
    expect(cps.map((c) => c.title)).toEqual(['Turn 1 recorded', 'Start job', 'Checkpoint 3'])
  })

  it('marks a failed stage on any row', () => {
    expect(fromLedger([row(1, { status: 'failed' }), row(2)], 'running')[0]?.status).toBe('failed')
  })

  it.each([
    ['running', 'running'],
    ['awaiting', 'running'],
    ['error', 'failed'],
    ['cancelled', 'stopped'],
    ['done', 'done'],
    [undefined, 'done'],
  ])('lets a run in state %s speak for its newest row only', (state, status) => {
    const cps = fromLedger([row(1), row(2)], state)
    expect(cps.map((c) => c.status)).toEqual(['done', status])
  })

  it('reads an empty ledger as no checkpoints', () => {
    expect(fromLedger([], 'running')).toEqual([])
  })
})

describe('clock', () => {
  it.each([
    [0, '0:00'],
    [61_000, '1:01'],
    [3_661_000, '1:01:01'],
    [-5, '0:00'],
  ])('formats %d', (ms, text) => {
    expect(clock(ms)).toBe(text)
  })
})

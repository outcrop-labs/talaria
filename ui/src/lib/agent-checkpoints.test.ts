import { describe, expect, it } from 'vitest'
import {
  clock,
  fromLineage,
  layout,
  lineageRewinding,
  livePath,
  path,
  plainFailure,
  plainReason,
  rewindConfirmation,
  rewindStageWords,
  runIsLive,
  summarize,
  toolKind,
  type Checkpoint,
  type LedgerRow,
  type Placed,
  type RewindRow,
  type RunAttempt,
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
  kind: 'turn',
  tool: null,
  title: `turn ${seq} recorded`,
  status: 'send',
  restorable: false,
  unrestorableReason: 'a turn boundary has no snapshot to restore',
  createdAt: new Date(1_800_000_000_000 + seq * 1000).toISOString(),
  ...extra,
})

const step = (seq: number, extra: Partial<LedgerRow> = {}) =>
  row(seq, { kind: 'step', title: `step ${seq}`, restorable: true, unrestorableReason: null, ...extra })

const attempt = (runId: string, attemptNo: number, rows: LedgerRow[], extra: Partial<RunAttempt> = {}): RunAttempt => ({
  runId,
  attemptNo,
  state: 'done',
  rewoundFromRunId: null,
  rewoundFromSeq: null,
  rows,
  ...extra,
})

const rewoundFrom = (runId: string, seq: number) => ({ rewoundFromRunId: runId, rewoundFromSeq: seq })

const rewindRow = (extra: Partial<RewindRow> = {}): RewindRow => ({
  id: 'w',
  state: 'running',
  stage: null,
  sourceRunId: 'r1',
  seq: 2,
  attemptRunId: null,
  error: null,
  ...extra,
})

const oneRun = (rows: LedgerRow[], state?: string) =>
  fromLineage([attempt('r1', 1, rows, { state })], [])

describe('layout', () => {
  it('draws no rows and one lane for an empty run', () => {
    expect(summarize(layout([]))).toEqual({ checkpoints: 0, attempts: 1, lanes: 1 })
  })

  it('keeps one attempt on one straight lane', () => {
    const placed = layout(line('a', 'b', 'c'))
    expect(placed.map((p) => p.lane)).toEqual([0, 0, 0])
    expect(placed.map((p) => p.attempt)).toEqual([undefined, undefined, undefined])
    expect(path(placedOf(placed, 'c') as Placed & { fromLane: number; fromMid: number })).not.toMatch(/C/)
  })

  it('draws a rewind out of its target on a lane of its own', () => {
    const placed = layout([...line('a', 'b', 'c'), cp('d', 'a', { attempt: 2 })])
    const fork = placedOf(placed, 'd')
    expect(fork.from).toBe(0)
    expect(fork.lane).not.toBe(fork.fromLane)
    expect(path(fork as Placed & { fromLane: number; fromMid: number })).toMatch(/C/)
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
    expect(placed.map((p) => p.attempt)).toEqual([1, 2, undefined])
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

  it.each(['constructor', 'mcp__talaria__constructor', '__proto__'])('reads %s as an unknown tool, not an object key', (name) => {
    expect(toolKind(name)).toBe('other')
  })

  it('does not match inside a word', () => {
    expect(toolKind('thread_summary')).toBe('other')
  })

  it('reads a row with no tool as a turn', () => {
    expect(toolKind(null)).toBe('turn')
  })
})

describe('one attempt', () => {
  it('chains each row to the one before it, timed from the first', () => {
    const cps = oneRun([row(1), row(2), row(4)])
    expect(cps.map((c) => c.parent)).toEqual([undefined, 'r1:1', 'r1:2'])
    expect(cps.map((c) => c.at)).toEqual([0, 1000, 3000])
  })

  it('reads as a sentence, and names a row that has no title', () => {
    const cps = oneRun([row(1), row(2, { title: null, tool: 'mcp__workbench__start_job' }), row(3, { title: null })])
    expect(cps.map((c) => c.title)).toEqual(['Turn 1 recorded', 'Start job', 'Checkpoint 3'])
  })

  it('marks a failed stage on any row', () => {
    expect(oneRun([row(1, { status: 'failed' }), row(2)], 'running')[0]?.status).toBe('failed')
  })

  it.each([
    ['running', 'running'],
    ['awaiting', 'running'],
    ['error', 'failed'],
    ['cancelled', 'stopped'],
    ['done', 'done'],
    [undefined, 'done'],
  ])('lets a run in state %s speak for its newest row only', (state, status) => {
    const cps = oneRun([row(1), row(2)], state)
    expect(cps.map((c) => c.status)).toEqual(['done', status])
  })

  it('keeps a failed stage on a head the run finished past', () => {
    expect(oneRun([row(1, { status: 'failed' })], 'done')[0]?.status).toBe('failed')
  })

  it('reads an empty ledger as no checkpoints', () => {
    expect(oneRun([], 'running')).toEqual([])
  })

  it('folds a turn answered and the next turn sent into one row', () => {
    const cps = oneRun([
      row(1, { title: 'pushed to atlas' }),
      step(2),
      row(3, { title: 'turn 1 answered', status: 'record' }),
      row(4, { title: 'turn 1 recorded', status: 'send' }),
      step(5),
    ])
    expect(cps.map((c) => c.title)).toEqual(['Pushed to atlas', 'Step 2', 'Turn 1 answered', 'Step 5'])
    expect(cps.at(-1)?.parent).toBe('r1:3')
  })

  it('numbers no attempt when there is only the one', () => {
    expect(oneRun([row(1)])[0]?.attempt).toBeUndefined()
  })
})

describe('a lineage', () => {
  const first = attempt('r1', 1, [row(1), step(2), step(3), step(4)])
  const second = attempt('r2', 2, [row(1), step(2)], rewoundFrom('r1', 3))
  const third = attempt('r3', 3, [row(1), step(2)], rewoundFrom('r1', 2))

  it('hangs an attempt off the row its server columns name', () => {
    const cps = fromLineage([first, second], [])
    const opener = cps.find((c) => c.id === 'r2:1')!
    expect(opener).toMatchObject({ parent: 'r1:3', fork: true, attempt: 2 })
  })

  it('orders attempts by the server number, not by how they arrived', () => {
    const ids = fromLineage([third, first, second], []).map((c) => c.id)
    expect(ids.indexOf('r1:1')).toBeLessThan(ids.indexOf('r2:1'))
    expect(ids.indexOf('r2:1')).toBeLessThan(ids.indexOf('r3:1'))
  })

  it('keeps a number the server skipped', () => {
    const cps = fromLineage([first, attempt('r9', 9, [row(1)], rewoundFrom('r1', 2))], [])
    expect(cps.at(-1)?.attempt).toBe(9)
  })

  it('leaves the rows past the rewind off the live path, and keeps them', () => {
    const placed = layout(fromLineage([first, second], []))
    const onPath = livePath(placed)
    const ids = (keep: (p: Placed) => boolean) => placed.filter(keep).map((p) => p.cp.id)
    expect(ids((p) => !onPath.has(p.row))).toEqual(['r1:4'])
    expect(placed).toHaveLength(6)
  })

  it('draws a second rewind of the left attempt on a lane of its own', () => {
    const placed = layout(fromLineage([first, second, third], []))
    const opener = placedOf(placed, 'r3:1')
    expect(opener.from).toBe(placedOf(placed, 'r1:2').row)
    expect(new Set([0, placedOf(placed, 'r2:1').lane, opener.lane]).size).toBe(3)
    const onPath = livePath(placed)
    expect(onPath.has(placedOf(placed, 'r2:2').row)).toBe(false)
    expect(onPath.has(placedOf(placed, 'r1:2').row)).toBe(true)
    expect(onPath.has(placedOf(placed, 'r1:3').row)).toBe(false)
  })

  it('still draws an attempt whose source has aged out of the history', () => {
    const placed = layout(fromLineage([second], []))
    expect(placedOf(placed, 'r2:1').from).toBeUndefined()
    expect(placedOf(placed, 'r2:1').attempt).toBe(2)
  })

  it('lets each attempt speak for its own head', () => {
    const left = attempt('r1', 1, [step(1), step(2)], { state: 'cancelled' })
    const cps = fromLineage([left, attempt('r2', 2, [row(1)], { state: 'running', ...rewoundFrom('r1', 1) })], [])
    expect(cps.map((c) => c.status)).toEqual(['done', 'stopped', 'running'])
  })
})

describe('what a row answers', () => {
  it('offers a rewind on a restorable step, aimed at its own run', () => {
    const cps = fromLineage([attempt('r1', 1, [step(2)]), attempt('r2', 2, [step(2)], rewoundFrom('r1', 2))], [])
    expect(cps.map((c) => c.answer)).toEqual([
      { kind: 'rewind', runId: 'r1', seq: 2 },
      { kind: 'rewind', runId: 'r2', seq: 2 },
    ])
  })

  it('offers no rewind on a turn row, even one marked restorable', () => {
    expect(oneRun([row(1, { restorable: true })])[0]?.answer?.kind).toBe('reason')
  })

  it.each([
    ['a turn boundary has no snapshot to restore', /turn boundary/i],
    ['compacted: the rows were archived (compacted = 1)', /compacted its conversation/],
    ['snapshot skipped: untracked content 20 MiB is over the 16 MiB snapshot budget', /snapshot budget/],
    ["the job's checkout was removed", /torn down/],
  ])('reads %s plainly', (reason, plain) => {
    const answer = oneRun([step(1, { restorable: false, unrestorableReason: reason })])[0]?.answer
    expect(answer).toMatchObject({ kind: 'reason', reason: expect.stringMatching(plain) })
  })

  it('never leaves a reason blank, and keeps one it has not met', () => {
    expect(plainReason(null)).toBe('This step kept no snapshot.')
    expect(plainReason('the disk filled up')).toBe('The disk filled up')
  })
})

describe('a rewind in flight', () => {
  const run = attempt('r1', 1, [step(1), step(2), step(3)])
  const mark = (rewinds: RewindRow[], id = 'r1:2') => fromLineage([run], rewinds).find((c) => c.id === id)?.rewind

  it.each(['queued', 'running', 'awaiting'])('reads %s as pending on its row, with the stage it reached', (state) => {
    expect(mark([rewindRow({ state, stage: 'restore' })])).toEqual({ kind: 'pending', stage: 'restore' })
  })

  it('marks only the step it names', () => {
    expect(mark([rewindRow()], 'r1:3')).toBeUndefined()
  })

  it('reports a live rewind for the lineage, and ignores another lineage', () => {
    expect(lineageRewinding([run], [rewindRow()])).toBe(true)
    expect(lineageRewinding([run], [rewindRow({ sourceRunId: 'other' })])).toBe(false)
    expect(lineageRewinding([run], [rewindRow({ state: 'error' })])).toBe(false)
  })

  it('words every stage, and the time before the first', () => {
    expect(rewindStageWords('fork')).toBe('Copying the conversation')
    expect(rewindStageWords(null)).toBe('Checking the step')
  })
})

describe('a failed rewind', () => {
  const run = attempt('r1', 1, [step(1), step(2)])
  const failed = rewindRow({ id: 'old', state: 'error', error: 'the agent did not answer' })

  it('reads failed on its row with its reason, and makes no attempt', () => {
    const cps = fromLineage([run], [failed])
    expect(cps.find((c) => c.id === 'r1:2')?.rewind).toEqual({ kind: 'failed', reason: 'The agent did not answer' })
    expect(cps).toHaveLength(2)
    expect(summarize(layout(cps)).attempts).toBe(1)
  })

  it('leaves the attempts as they were', () => {
    const { rewind: _, ...before } = fromLineage([run], [])[1]!
    const { rewind: __, ...after } = fromLineage([run], [failed])[1]!
    expect(after).toEqual(before)
  })

  it('says plainly when the agent is down', () => {
    const error = 'the agent cannot rewind: in c-1: Error response from daemon: container 75954b is not running'
    expect(plainFailure(error)).toBe('The agent is not running, so it could not rewind. Start it and try again.')
  })

  it('reads a cancelled rewind as failed too, with a reason', () => {
    expect(fromLineage([run], [rewindRow({ state: 'cancelled' })])[1]?.rewind).toMatchObject({ kind: 'failed' })
  })

  it('gives way to a newer rewind of the same step', () => {
    const landed = rewindRow({ id: 'new', state: 'done' })
    const retrying = rewindRow({ id: 'new', state: 'running' })
    expect(fromLineage([run], [landed, failed])[1]?.rewind).toBeUndefined()
    expect(fromLineage([run], [retrying, failed])[1]?.rewind?.kind).toBe('pending')
  })
})

describe('the confirm step', () => {
  const outside = {
    tokens: 12_345,
    pushes: [
      { branch: 'agent/a', at: '2026-11-04T10:00:00Z' },
      { branch: 'agent/a', at: '2026-11-04T10:01:00Z' },
    ],
    pullRequests: ['https://forge.test/pr/1'],
  }

  it('says everything that stays changed or is not restored', () => {
    const text = rewindConfirmation({ running: false, outside, ticketMove: null })
    expect(text).toContain('12,345 tokens spent')
    expect(text.match(/Pushes to agent\/a/g)).toHaveLength(1)
    expect(text).toContain('Pull request https://forge.test/pr/1')
    expect(text).toContain('Agent memory is not restored')
    expect(text).toContain('coding harness session starts fresh')
  })

  it('warns of a stop only for a run that is working', () => {
    expect(rewindConfirmation({ running: true, ticketMove: null })).toContain('will stop first')
    expect(rewindConfirmation({ running: false, ticketMove: null })).not.toContain('stop')
  })

  it('names the ticket move only when there is one', () => {
    const move = { from: 'In review', to: 'In progress' }
    expect(rewindConfirmation({ running: false, ticketMove: move })).toContain('moves from In review back to In progress')
    expect(rewindConfirmation({ running: false, ticketMove: null })).not.toContain('ticket')
  })
})

describe('heights', () => {
  it('moves everything under an open note down by its height', () => {
    const placed = layout(line('a', 'b', 'c'), { row: 44, notes: { b: 20 } })
    expect(placed.map((p) => p.top)).toEqual([0, 44, 108])
    expect(placed.map((p) => p.mid)).toEqual([22, 66, 130])
    expect(path(placedOf(placed, 'c') as Placed & { fromLane: number; fromMid: number })).toBe('M22 66V130')
  })
})

describe('runIsLive', () => {
  it.each([
    ['running', true],
    ['awaiting', true],
    ['queued', true],
    ['done', false],
    ['error', false],
    ['cancelled', false],
    [undefined, false],
  ])('reads %s as %s', (state, live) => {
    expect(runIsLive(state)).toBe(live)
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

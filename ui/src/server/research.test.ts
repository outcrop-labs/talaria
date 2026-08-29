// THE ADAPTER, and the sweep that is not there any more.
//
// What this file used to assert — that a synthesis stage which never reached
// the persona leaves no artifact and puts the gateway's sentence on the run —
// moved with the pipeline to `runs/defs/research.test.ts`, which asserts it
// against the definition that now owns it. What is left in this module is the
// half that never belonged to the pipeline: the domain record, the reads a
// surface makes, and the four statements that start a run.
//
// Two of those are regressions worth pinning for good:
//
//   THE RUN ROW GOES IN FIRST. It carries the question, the mode and the owner
//   on its `input`, so it is the record that can be reclaimed; a research row
//   written first would be a row nobody is driving, and there is no stale sweep
//   left to notice one.
//
//   READING DOES NOT WRITE. `listResearchRuns` and `getResearchRun` both used
//   to call `sweepStale()`, so opening the research page was what marked a run
//   that had outlived a deploy FAILED. Both reads are now reads.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const queries: Array<{ text: string; values: unknown[] }> = []

const RUN = {
  id: 'the-id',
  status: 'queued',
  mode: 'recon',
  question: 'what changed in postgres 17',
  agentModel: 'nomad',
  ownerUserId: 'user-1',
  requestedBy: 'user-1',
}

const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  return Promise.resolve([])
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  json: (v: unknown) => unknown
  unsafe: (text: string, values?: unknown[]) => unknown
}
sql.json = (v: unknown) => v
// Two jobs, as in postgres.js: a bare fragment inside a template, and a whole
// statement (every read in this module goes through `unsafe` because the
// projection is a shared column list).
sql.unsafe = (text: string, values?: unknown[]) => {
  if (!/^\s*select/i.test(text)) return text
  queries.push({ text: text.replace(/\s+/g, ' ').trim(), values: values ?? [] })
  return Promise.resolve([RUN])
}

const enqueue = vi.fn(async () => ({ id: 'the-id' }))
const drive = vi.fn(async () => ({}))
// Pushes a sentinel into the same log the SQL goes into, so the ORDER of
// "cancel the run" against "delete the record" is observable — which is the
// half of `deleteResearchRun` that matters.
const cancelRun = vi.fn(async () => {
  queries.push({ text: '-- cancelRun', values: [] })
  return { ok: true as const, state: 'cancelled' as const }
})
const planSearch = vi.fn(async (_mode: string): Promise<{ model: string; via: 'native' | 'tool'; supplier: { server: string; tool: string } | null } | null> => ({
  model: 'sonar',
  via: 'native',
  supplier: null,
}))

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/runs/run', () => ({ enqueue, drive, cancelRun }))
vi.mock('@/server/runs/defs/research', () => ({
  RESEARCH_MODES: [{ mode: 'recon', blurb: 'one fast pass' }],
  NO_SEARCH_REASON: 'this workspace cannot search yet',
  planSearch: (mode: string) => planSearch(mode),
  researchRun: { kind: 'research' },
}))
vi.mock('@/server/titler', () => ({ generateTitle: async () => null }))

const { activeResearchOn, briefableResearch, deleteResearchRun, getResearchRun, listResearchRuns, startResearch } = await import('@/server/research')

const writes = () => queries.filter((q) => /^\s*(update|insert|delete)/i.test(q.text))

beforeEach(() => {
  queries.length = 0
  enqueue.mockClear()
  drive.mockClear()
  cancelRun.mockClear()
  planSearch.mockResolvedValue({ model: 'sonar', via: 'native', supplier: null })
})

describe('starting a run', () => {
  it('writes the RUN first, then the record it is about, under one id', async () => {
    const run = await startResearch({ question: RUN.question, mode: 'recon', agentModel: 'nomad', ownerUserId: 'user-1', requestedBy: 'user-1' })
    expect(run.id).toBe('the-id')

    expect(enqueue).toHaveBeenCalledTimes(1)
    const [, input, opts] = enqueue.mock.calls[0] as unknown as [unknown, Record<string, unknown>, Record<string, unknown>]
    // Everything the pipeline needs to rebuild the domain record after a crash —
    // `parentRunId` included, because a follow-up that loses it on a reclaim
    // restarts its citation numbering at [1] and re-aims the parent's prose.
    expect(input).toEqual({
      question: RUN.question,
      mode: 'recon',
      agentModel: 'nomad',
      ownerUserId: 'user-1',
      requestedBy: 'user-1',
      parentRunId: null,
    })
    // RISK 6: a deterministic id, so a retried call collides on the primary key
    // instead of starting a second run doing the same work.
    expect(opts.id).toEqual(expect.any(String))
    expect(opts.subjectType).toBe('research')
    expect(opts.subjectId).toBe(opts.id)
    expect(opts.ownerUserId).toBe('user-1')
    // The drive is this module's, so the record exists before a step can look
    // for it — the reclaim sweep is what makes it finish either way.
    expect(opts.start).toBe(false)
    expect(drive).toHaveBeenCalledWith(opts.id)

    const insert = queries.find((q) => q.text.includes('insert into research_runs'))
    expect(insert).toBeDefined()
    expect(insert?.values[0]).toBe(opts.id)
    // The insert cannot lose a race with the run's own `ensureRow`.
    expect(insert?.text).toContain('on conflict (id) do nothing')
  })

  it('refuses up front when the workspace cannot search, and starts nothing', async () => {
    planSearch.mockResolvedValue(null)
    await expect(
      startResearch({ question: RUN.question, mode: 'recon', agentModel: 'nomad', ownerUserId: 'user-1', requestedBy: 'user-1' }),
    ).rejects.toThrow(/this workspace cannot search yet/)
    expect(enqueue).not.toHaveBeenCalled()
    expect(writes()).toEqual([])
  })
})

describe('reading', () => {
  it('never writes — the stale sweep is gone from both reads', async () => {
    await listResearchRuns('user-1')
    await listResearchRuns(null)
    await getResearchRun('the-id')
    await activeResearchOn(RUN.question)
    expect(writes()).toEqual([])
    // And in particular, the sentence this whole port exists to delete.
    expect(queries.some((q) => q.text.includes('went stale'))).toBe(false)
  })

  it('projects status and phase from the run, not from the record', async () => {
    await getResearchRun('the-id')
    const read = queries.find((q) => q.text.includes('from research_runs'))
    // One authority on whether a run is alive: the `runs` row, joined on the id
    // both records share. The research row's own columns are the fallback for
    // rows written before there were runs.
    expect(read?.text).toContain("left join runs r on r.id = research_runs.id and r.kind = 'research'")
    expect(read?.text).toContain('when r.state is null then research_runs.status')
    expect(read?.text).toContain("when r.state in ('running', 'awaiting') then 'running'")
  })

  it('carries the question a parked run is waiting on — and only while it is open', async () => {
    await getResearchRun('the-id')
    const read = queries.find((q) => q.text.includes('from research_runs'))
    // The four-value wire keeps a parked run 'running'; `awaiting` is what
    // makes it LOOK parked on the surface that can answer it. An answered
    // decision never renders as if it were still open — the answer-null guard
    // is the SQL spelling of `pendingQuestion`'s rule in runs/decide.ts.
    expect(read?.text).toContain("case when r.state = 'awaiting' and r.decision->'request' is not null and r.decision->'answer' is null")
    expect(read?.text).toContain(`then r.decision->'request' else null end as "awaiting"`)
  })

  it('answers "is this question already being researched" from the run', async () => {
    await activeResearchOn(RUN.question)
    const read = queries.find((q) => q.text.includes('left join runs r'))
    // The raw `status in ('queued','running')` this replaces reads a column
    // that no longer moves when a DRIVER gives up on a run, so a question could
    // become unaskable for ever.
    expect(read?.text).toContain("else r.state in ('queued', 'running', 'awaiting') end")
  })
})

describe('deleting a run', () => {
  it('CANCELS THE RUN FIRST, then deletes the record', async () => {
    // Deleting alone stopped nothing: the work lives on the `runs` row, and the
    // driver holding it went on planning, searching and synthesizing a report
    // for a record that no longer existed. The order matters as much as the
    // call — the other way round leaves a window where the record is gone and
    // the run is still `running`, which is exactly what a reclaim sweep picks
    // up.
    await deleteResearchRun('the-id')
    expect(cancelRun).toHaveBeenCalledWith({ runId: 'the-id', reason: expect.stringContaining('deleted') })
    const order = queries.map((q) => (q.text === '-- cancelRun' ? 'cancel' : /^\s*delete/i.test(q.text) ? 'delete' : 'other'))
    expect(order.filter((o) => o !== 'other')).toEqual(['cancel', 'delete'])
  })
})

describe('the briefing', () => {
  it('projects from the run rather than filtering the record\u2019s own status', async () => {
    // briefing.ts used to ask `where status in ('queued','running')` itself. A
    // run a driver gave up on keeps that column at 'queued' for ever, so the
    // person got "research queued: ..." in their briefing every morning for a
    // run that stopped weeks ago.
    await briefableResearch('user-1')
    const read = queries.find((q) => q.text.includes('from research_runs'))
    expect(read?.text).toContain("left join runs r on r.id = research_runs.id and r.kind = 'research'")
    expect(read?.text).toContain("when r.state in ('running', 'awaiting') then 'running'")
    // The filter runs over the PROJECTED status, not the stored one.
    expect(read?.text).toContain("where s.status in ('queued', 'running')")
    expect(writes()).toEqual([])
  })
})

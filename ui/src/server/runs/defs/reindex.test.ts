import { describe, expect, it } from 'vitest'
import {
  BACKFILL_SOURCES,
  stepBackfillRun,
  stepReindex,
  type BackfillCheckpoint,
  type BackfillDeps,
  type ReindexCheckpoint,
  type ReindexDeps,
  type RegisteredCollection,
} from '@/server/runs/defs/reindex'
import type { RunRow, RunStepContext, StepResult } from '@/server/runs/define'

// The two retrieval repair runs, driven with no Postgres, no Qdrant and no
// embedding service — every edge either step touches is a field on `BackfillDeps`
// or `ReindexDeps`. Same pattern and same reason as server/runs/run.test.ts.
//
// WHAT THESE ASSERT is not "the steps run". It is the two properties that the
// pre-run code did not have and that a reader cannot check by eye:
//
//   A KILLED RUN RESUMES AT THE NEXT UNIT. The checkpoint is re-entered, the
//   completed pages are not re-fetched, and the source behind the cursor is
//   never revisited.
//
//   A RECLAIM DOES NOT RESTART THE REBUILD. This is the most destructive
//   sequence in the product — DROP a collection, purge its bookkeeping, refill —
//   and the whole guard is that the `rebuilding → backfilling` flip happens in a
//   step with no outward effect in it. There is a test below for exactly that
//   step, because a future edit that folds it back into the last rebuild would
//   look like a simplification and would silently reintroduce "re-drop the
//   collection the backfill has already refilled".

const ISO = '2026-01-01T00:00:00.000Z'

function ctxOf<I, C>(over: { input: I; checkpoint?: C | null; attempt?: number; signal?: AbortSignal }): RunStepContext<I, C> {
  const attempt = over.attempt ?? 0
  const run: RunRow = {
    id: 'run-1',
    kind: 'test',
    ownerUserId: null,
    subjectType: null,
    subjectId: null,
    state: 'running',
    phase: '',
    checkpoint: over.checkpoint ?? null,
    input: over.input,
    result: null,
    error: null,
    attempt,
    leaseOwner: 'token',
    leaseExpiresAt: null,
    approvalKey: null,
    decision: null,
    createdAt: ISO,
    updatedAt: ISO,
    startedAt: ISO,
    finishedAt: null,
  }
  return {
    run,
    input: over.input,
    checkpoint: over.checkpoint ?? null,
    decision: null,
    signal: over.signal ?? new AbortController().signal,
    log: () => {},
    attempt,
  }
}

const UP = async () => ({ qdrant: true, embeddings: true })

/** Drive a step to completion the way `drive()` does — re-entering with the
 *  checkpoint the last one PERSISTED — and hand back every result in order so a
 *  test can assert on the sequence rather than only the end. */
async function driveAll<C>(
  step: (checkpoint: C | null, attempt: number) => Promise<StepResult<C>>,
  opts: { max?: number; attempt?: number } = {},
): Promise<{ results: Array<StepResult<C>>; last: C | null }> {
  const results: Array<StepResult<C>> = []
  let checkpoint: C | null = null
  for (let i = 0; i < (opts.max ?? 100); i++) {
    const res = await step(checkpoint, opts.attempt ?? 0)
    results.push(res)
    if (res.kind !== 'next') return { results, last: checkpoint }
    checkpoint = res.checkpoint
  }
  throw new Error('the step never settled')
}

// ── The backfill ─────────────────────────────────────────────────────────────

describe('backfill paging', () => {
  /** A fake corpus: every source has `rows` ids, handed out a page at a time. */
  const corpus = (rows: Record<string, string[]>, seen: string[]): Partial<BackfillDeps> => ({
    health: UP,
    page: async (source, cursor, counts) => {
      const all = rows[source] ?? []
      const from = cursor === null ? 0 : all.indexOf(cursor) + 1
      const page = all.slice(from, from + 2)
      for (const id of page) seen.push(`${source}:${id}`)
      const last = page[page.length - 1] ?? cursor
      return { done: page.length < 2, cursor: last, counts: page.length ? { ...counts, [source]: (counts[source] ?? 0) + page.length } : counts }
    },
  })

  /** Start partway in, at the first paged source. `collections` is the sequence's
   *  own preamble (it ensures the Qdrant collections exist) and pages nothing. */
  const atKbDocs = (counts: Record<string, number> = {}): BackfillCheckpoint => ({ source: 'kb-docs', cursor: null, counts })

  it('walks every source in order and finishes', async () => {
    const seen: string[] = []
    const deps = corpus({ 'kb-docs': ['a', 'b', 'c'], tickets: ['t1'] }, seen)
    const { results } = await driveAll<BackfillCheckpoint>((cp) => stepBackfillRun(ctxOf({ input: {}, checkpoint: cp }), deps))
    const done = results[results.length - 1]
    expect(done?.kind).toBe('done')
    expect(seen).toEqual(['kb-docs:a', 'kb-docs:b', 'kb-docs:c', 'tickets:t1'])
    expect(done?.kind === 'done' && (done.result as { counts: Record<string, number> }).counts).toEqual({ 'kb-docs': 3, tickets: 1 })
  })

  it('RESUMES AT THE NEXT UNIT — a killed page re-enters at the cursor, not at the start of the source', async () => {
    const first: string[] = []
    const rows = { 'kb-docs': ['a', 'b', 'c', 'd', 'e'] }
    // Two pages, then the process dies. The checkpoint of the SECOND page is
    // the last one persisted.
    let cp: BackfillCheckpoint | null = atKbDocs()
    for (let i = 0; i < 2; i++) {
      const res = await stepBackfillRun(ctxOf({ input: {}, checkpoint: cp }), corpus(rows, first))
      if (res.kind !== 'next') throw new Error('expected progress')
      cp = res.checkpoint
    }
    expect(first).toEqual(['kb-docs:a', 'kb-docs:b', 'kb-docs:c', 'kb-docs:d'])

    // The reclaim: a NEW driver, re-entered with that checkpoint.
    const after: string[] = []
    const res = await stepBackfillRun(ctxOf({ input: {}, checkpoint: cp, attempt: 1 }), corpus(rows, after))
    expect(res.kind).toBe('next')
    // Only what was left. Nothing behind the cursor is re-fetched.
    expect(after).toEqual(['kb-docs:e'])
  })

  it('carries the tally across the resume rather than starting it again', async () => {
    const rows = { 'kb-docs': ['a', 'b', 'c'] }
    const one = await stepBackfillRun(ctxOf({ input: {}, checkpoint: atKbDocs() }), corpus(rows, []))
    if (one.kind !== 'next') throw new Error('expected progress')
    expect(one.checkpoint.counts).toEqual({ 'kb-docs': 2 })
    const two = await stepBackfillRun(ctxOf({ input: {}, checkpoint: one.checkpoint, attempt: 3 }), corpus(rows, []))
    if (two.kind !== 'next') throw new Error('expected progress')
    expect(two.checkpoint.counts).toEqual({ 'kb-docs': 3 })
  })

  it('WAITS for a dead Qdrant instead of failing the run', async () => {
    // The pre-run code threw here, filed the backfill as errored, and left the
    // admin to notice the services came back and press the button again.
    const res = await stepBackfillRun(ctxOf<Record<string, never>, BackfillCheckpoint>({ input: {}, checkpoint: null }), {
      health: async () => ({ qdrant: false, embeddings: true }),
      page: async () => {
        throw new Error('must not page while the services are down')
      },
    })
    expect(res.kind).toBe('retry')
    expect(res.kind === 'retry' && res.reason).toContain('qdrant: down')
  })

  it('probes health at a source boundary only, not on every page', async () => {
    let probes = 0
    const rows = { 'kb-docs': ['a', 'b', 'c'] }
    const deps: Partial<BackfillDeps> = {
      ...corpus(rows, []),
      health: async () => {
        probes++
        return { qdrant: true, embeddings: true }
      },
    }
    await driveAll<BackfillCheckpoint>((cp) => stepBackfillRun(ctxOf({ input: {}, checkpoint: cp }), deps))
    // One per source, and never once per page — the probe costs an embedding
    // call and paying for one per hundred documents is a tax on the healthy path.
    expect(probes).toBe(BACKFILL_SOURCES.length)
  })
})

// ── The rebuild ──────────────────────────────────────────────────────────────

describe('reindex rebuild', () => {
  const COLS: RegisteredCollection[] = [
    { id: 'c1', qdrantName: 'talaria_activity' },
    { id: 'c2', qdrantName: 'talaria_org_kb' },
    { id: 'c3', qdrantName: 'talaria_custom' },
  ]

  const deps = (dropped: string[], over: Partial<ReindexDeps> = {}): Partial<ReindexDeps> => ({
    embedDim: async () => 1024,
    collections: async () => COLS,
    rebuild: async (col) => void dropped.push(col.id),
    invalidate: () => {},
    // The backfill half is finished immediately: this suite is about the rebuild.
    backfill: { health: UP, page: async (_s, _c, counts) => ({ done: true, cursor: null, counts }) },
    ...over,
  })

  it('rebuilds one collection per step, then flips phase, then refills', async () => {
    const dropped: string[] = []
    const { results } = await driveAll<ReindexCheckpoint>((cp) => stepReindex(ctxOf({ input: {}, checkpoint: cp }), deps(dropped)))
    expect(dropped).toEqual(['c1', 'c2', 'c3'])
    expect(results[results.length - 1]?.kind).toBe('done')
  })

  it('THE PHASE FLIP IS A STEP OF ITS OWN, with no rebuild in it', async () => {
    // This is the whole re-entry argument. If a future edit folds the flip into
    // the last rebuild step to save a write, a crash in that window re-enters
    // the rebuild and drops collections the backfill has already refilled.
    const dropped: string[] = []
    let cp: ReindexCheckpoint | null = null
    const step = async () => {
      const res = await stepReindex(ctxOf({ input: {}, checkpoint: cp }), deps(dropped))
      if (res.kind !== 'next') throw new Error('expected progress')
      cp = res.checkpoint
      return res
    }
    await step()
    await step()
    const third = await step()
    expect(third.checkpoint.rebuilt).toEqual(['c1', 'c2', 'c3'])
    expect(third.checkpoint.phase).toBe('rebuilding')
    expect(dropped).toEqual(['c1', 'c2', 'c3'])

    const flip = await step()
    expect(flip.checkpoint.phase).toBe('backfilling')
    // Nothing was dropped by the flip.
    expect(dropped).toEqual(['c1', 'c2', 'c3'])
  })

  it('A RECLAIM DOES NOT RESTART THE REBUILD — collections already rebuilt are not dropped again', async () => {
    const before: string[] = []
    let cp: ReindexCheckpoint | null = null
    for (let i = 0; i < 2; i++) {
      const res = await stepReindex(ctxOf({ input: {}, checkpoint: cp }), deps(before))
      if (res.kind !== 'next') throw new Error('expected progress')
      cp = res.checkpoint
    }
    expect(before).toEqual(['c1', 'c2'])

    // The driver died. A new one re-enters from the persisted checkpoint.
    const after: string[] = []
    const { results } = await driveAll<ReindexCheckpoint>((c, attempt) => stepReindex(ctxOf({ input: {}, checkpoint: c ?? cp, attempt }), deps(after)), {
      attempt: 1,
    })
    expect(after).toEqual(['c3'])
    expect(results[results.length - 1]?.kind).toBe('done')
  })

  it('a reclaim during the BACKFILL phase never touches the rebuild', async () => {
    // The failure this rules out: the phase write landed, the process died
    // during the refill, and the re-entry re-drops a half-filled index.
    const dropped: string[] = []
    const mid: ReindexCheckpoint = { phase: 'backfilling', rebuilt: ['c1', 'c2', 'c3'], embedDim: 1024, backfill: { source: 'tickets', cursor: 't7', counts: {} } }
    await driveAll<ReindexCheckpoint>((cp, attempt) => stepReindex(ctxOf({ input: {}, checkpoint: cp ?? mid, attempt }), deps(dropped)), { attempt: 2 })
    expect(dropped).toEqual([])
  })

  it('starts the rebuild over when the embedding dimension changes under it', async () => {
    // A dimension change is what this run exists to repair, so one DURING it
    // means the collections already rebuilt are in the wrong shape. Finishing
    // would leave half the plane at 384 and half at 1024, each failing every
    // call against the other.
    const dropped: string[] = []
    const mid: ReindexCheckpoint = { phase: 'rebuilding', rebuilt: ['c1', 'c2'], embedDim: 384, backfill: null }
    const res = await stepReindex(ctxOf({ input: {}, checkpoint: mid }), deps(dropped))
    expect(res.kind).toBe('next')
    expect(res.kind === 'next' && res.checkpoint).toMatchObject({ rebuilt: [], embedDim: 1024, phase: 'rebuilding' })
    // The step that notices is pure: nothing is dropped by it.
    expect(dropped).toEqual([])
  })

  it('WAITS for the embedding service instead of failing the rebuild', async () => {
    const dropped: string[] = []
    const res = await stepReindex(ctxOf<Record<string, never>, ReindexCheckpoint>({ input: {}, checkpoint: null }), deps(dropped, { embedDim: async () => null }))
    expect(res.kind).toBe('retry')
    expect(dropped).toEqual([])
  })

  it('does not start a rebuild the driver has already given up on', async () => {
    const dropped: string[] = []
    const ac = new AbortController()
    ac.abort()
    const res = await stepReindex(ctxOf<Record<string, never>, ReindexCheckpoint>({ input: {}, checkpoint: null, signal: ac.signal }), deps(dropped))
    // Rule 2 of the at-least-once checklist: a step abandoned by the driver may
    // still be running on the instance that abandoned it, so it must not make an
    // outward call it has not been asked for.
    expect(res.kind).toBe('retry')
    expect(dropped).toEqual([])
  })
})

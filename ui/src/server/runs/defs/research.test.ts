// What a research run is worth is what it keeps when the process dies, so that
// is what this file measures: every test below kills a driver somewhere and
// then asks what the resumed run PAID for and what it PRODUCED.
//
// THE DRIVER IS SIMULATED, not mocked. `step()` is the definition's whole
// contract with runs/run.ts, and the two rules that make an assertion here mean
// anything are the two the real driver follows: a `next` result's checkpoint is
// what the NEXT entry is given, and a `decide` result's answer is cleared in
// the same write as the checkpoint the consuming step produced. `killAt` is the
// only interesting third thing — it drops a checkpoint on the floor, which is
// exactly what a crash between the step and `store.checkpoint` does, and it is
// how the at-least-once cost of this port is stated as a number rather than as
// a paragraph.
import { describe, expect, it, vi } from 'vitest'
import type { DecisionAnswer, RunRow, StepResult } from '@/server/runs/define'
import { SourceRegistry, type ResearchSource } from '@/server/source-registry'
import {
  makeResearchRun,
  type ResearchCheckpoint,
  type ResearchInput,
  type ResearchRunDeps,
} from '@/server/runs/defs/research'

// Everything this module registers at import time is a definition and a Map
// entry; the scheduler is not in that graph, but `runs/reclaim.ts` may become a
// sibling import later and it registers a job.
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))

// ── The fake world ───────────────────────────────────────────────────────────

interface World {
  deps: ResearchRunDeps
  /** A follow-up's parent sources, seeded into the registry at `begin`. */
  parentSources: Array<{ idx: number; url: string; title: string | null; snippet: string | null }>
  /** Every query actually sent to a search model, in order. Its LENGTH is the
   *  bill: one entry is one paid sonar call. */
  searched: string[]
  /** The supplier each search call was handed — the plan's path, threaded
   *  through the checkpoint. Null entries are native searches. */
  suppliers: Array<{ server: string; tool: string } | null>
  planned: number
  synthesized: number
  /** Artifacts created. More than one is the failure this port exists to make
   *  impossible. */
  created: string[]
  /** Bodies written, by artifact id. */
  written: Array<{ artifactId: string; body: string }>
  indexed: number
  notified: number
  finished: Array<{ artifactId: string; stats: Record<string, number> }>
  failed: string[]
  sourcesSaved: ResearchSource[][]
  /** The artifact_links row, which is what makes a created artifact findable by
   *  the next entry. */
  link: string | null
  rowExists: boolean
  /** Queries whose search should throw, by query text. */
  deadQueries: Set<string>
  allSearchesDead: boolean
  /** With allSearchesDead: how many searches stay dead before the outage
   *  passes. unset = dead for the whole run. */
  reviveAfter: number | null
}

function makeWorld(over: Partial<ResearchRunDeps> = {}): World {
  let artifactSeq = 0
  const w: World = {
    searched: [],
    suppliers: [],
    planned: 0,
    synthesized: 0,
    created: [],
    written: [],
    indexed: 0,
    notified: 0,
    finished: [],
    failed: [],
    sourcesSaved: [],
    parentSources: [],
    link: null,
    rowExists: true,
    deadQueries: new Set(),
    allSearchesDead: false,
    reviveAfter: null,
    deps: null as unknown as ResearchRunDeps,
  }
  w.deps = {
    searchPlanFor: async () => ({ model: 'sonar-pro', via: 'native' as const, supplier: null }),
    // A follow-up's parent sources. Empty unless a test sets `w.parentSources`,
    // which is what the numbering-continues case does.
    sourcesOf: async () => w.parentSources,
    planQueries: async ({ max }) => {
      w.planned++
      return Array.from({ length: max }, (_, i) => `angle ${w.planned}.${i + 1}`)
    },
    search: async ({ query, supplier }) => {
      w.searched.push(query)
      w.suppliers.push(supplier)
      // `reviveAfter`: a transient outage — dead for the first N searches of
      // the run, healthy from the next one on. What the retry has to survive.
      const dead = w.allSearchesDead && (w.reviveAfter === null || w.searched.length <= w.reviveAfter)
      if (dead || w.deadQueries.has(query)) throw new Error(`search stage 502 on "${query}"`)
      return {
        content: `findings for ${query} [1]`,
        sources: [{ url: `https://example.com/${encodeURIComponent(query)}`, title: query, snippet: 's' }],
      }
    },
    synthesize: async () => {
      w.synthesized++
      return { doc: '# A report\n\nthe vendor published a SOC 2 Type II [1]', ungrounded: 0 }
    },
    agentLabel: (m) => m,
    ensureRow: async () => {
      w.rowExists = true
    },
    rowExists: async () => w.rowExists,
    memberIds: async () => ['member-1'],
    saveSources: async (_id, sources) => {
      w.sourcesSaved.push(sources)
    },
    finishRow: async ({ artifactId, stats }) => {
      w.finished.push({ artifactId, stats })
    },
    failRow: async (_id, error) => {
      w.failed.push(error)
    },
    linkedArtifact: async () => w.link,
    createReport: async () => {
      const id = `art-${++artifactSeq}`
      w.created.push(id)
      // The real `createReport` links in the same breath as it creates, which is
      // what makes the id addressable by the next entry. The fake does the same,
      // or the "two artifacts" test would be testing the fake.
      w.link = id
      return id
    },
    writeReport: async ({ artifactId, body }) => {
      w.written.push({ artifactId, body })
    },
    index: async () => {
      w.indexed++
    },
    notify: async () => {
      w.notified++
    },
    ...over,
  }
  return w
}

const INPUT: ResearchInput = {
  question: 'what changed in postgres 17',
  mode: 'brief',
  agentModel: 'nomad',
  ownerUserId: 'user-1',
  requestedBy: 'user-1',
}

function rowFor(input: ResearchInput, over: Partial<RunRow> = {}): RunRow {
  const now = new Date('2026-08-06T00:00:00.000Z').toISOString()
  return {
    id: 'run-1',
    kind: 'research',
    ownerUserId: input.ownerUserId,
    subjectType: 'research',
    subjectId: 'run-1',
    state: 'running',
    phase: 'queued',
    checkpoint: null,
    input,
    result: null,
    error: null,
    attempt: 0,
    leaseOwner: 'token',
    leaseExpiresAt: null,
    approvalKey: null,
    decision: null,
    createdAt: now,
    updatedAt: now,
    startedAt: now,
    finishedAt: now,
    ...over,
  }
}

interface DriveOptions {
  /** Drop the checkpoint produced by the Nth step (1-based) — the crash between
   *  a step returning and `store.checkpoint` landing. The run then re-enters
   *  with the PREVIOUS checkpoint, which is the at-least-once contract. */
  killAt?: number
  /** Answer the run's question the moment it parks, once. */
  answer?: (question: { key: string; options: Array<{ id: string }> }) => DecisionAnswer | null
  maxSteps?: number
}

interface DriveOutcome {
  steps: number
  stop: 'done' | 'awaiting' | 'error'
  result?: unknown
  error?: string
  question?: { key: string; question: string; options: Array<{ id: string }> }
  checkpoint: ResearchCheckpoint | null
  /** Attempts entered, as the driver counts them: one per re-entry. */
  attempt: number
}

/** The driver's loop, honestly: persist the checkpoint, then the next step. */
async function driveRun(world: World, input: ResearchInput, opts: DriveOptions = {}): Promise<DriveOutcome> {
  const def = makeResearchRun(world.deps)
  let checkpoint: ResearchCheckpoint | null = null
  let decision: DecisionAnswer | null = null
  let attempt = 0
  let steps = 0
  const max = opts.maxSteps ?? 200
  const ac = new AbortController()

  for (;;) {
    if (steps >= max) throw new Error(`the run did not stop after ${max} steps — the budget does not bound the loop`)
    const row = rowFor(input, { checkpoint, attempt, ...(decision ? { decision: { request: { key: decision.key, question: '', options: [] }, answer: decision } } : {}) })
    let result: StepResult<ResearchCheckpoint>
    steps++
    try {
      result = await def.step({ run: row, input, checkpoint, decision, signal: ac.signal, log: () => {}, attempt })
    } catch (e) {
      return { steps, stop: 'error', error: (e as Error).message, checkpoint, attempt }
    }
    if (result.kind === 'done') return { steps, stop: 'done', result: result.result, checkpoint, attempt }
    if (result.kind === 'decide') {
      const answer = opts.answer?.(result.question) ?? null
      if (!answer) return { steps, stop: 'awaiting', question: result.question, checkpoint, attempt }
      // `store.answer` puts the answer on the row and requeues; the next step
      // reads it as `ctx.decision`.
      decision = answer
      continue
    }
    if (result.kind === 'retry') throw new Error('research does not defer')
    if (opts.killAt === steps) {
      // THE CRASH. The checkpoint never landed, so the next driver re-enters
      // with the one before it — and `attempt` moves, which is the only thing
      // that tells the step it has been here before.
      attempt++
      continue
    }
    checkpoint = result.checkpoint
    // Cleared in the SAME write as the checkpoint the consuming step produced
    // (store.checkpoint's `clearDecision`), which is what stops a decision
    // being acted on twice.
    decision = null
  }
}

// ── The budgets still bound the loop ─────────────────────────────────────────

describe('the mode budgets', () => {
  it('recon asks the question itself: one search, no planner call', async () => {
    const w = makeWorld()
    const out = await driveRun(w, { ...INPUT, mode: 'recon' })
    expect(out.stop).toBe('done')
    expect(w.planned).toBe(0)
    expect(w.searched).toEqual(['what changed in postgres 17'])
  })

  it('brief plans once and runs three queries', async () => {
    const w = makeWorld()
    const out = await driveRun(w, { ...INPUT, mode: 'brief' })
    expect(out.stop).toBe('done')
    expect(w.planned).toBe(1)
    expect(w.searched).toHaveLength(3)
  })

  it('expedition runs three rounds of four and then stops on its own', async () => {
    const w = makeWorld()
    const out = await driveRun(w, { ...INPUT, mode: 'expedition' })
    expect(out.stop).toBe('done')
    expect(w.planned).toBe(3)
    expect(w.searched).toHaveLength(12)
    expect(w.synthesized).toBe(1)
  })

  it('shrinks the loop for a deep-research search model, as adaptBudget always did', async () => {
    const w = makeWorld({ searchPlanFor: async () => ({ model: 'perplexity/sonar-deep-research', via: 'native' as const, supplier: null }) })
    await driveRun(w, { ...INPUT, mode: 'expedition' })
    // rounds min(3,2) = 2, one query each.
    expect(w.planned).toBe(2)
    expect(w.searched).toHaveLength(2)
  })

  it("carries the plan's supplier to every search call — a tool plan must not silently run native", async () => {
    // The regression this pins: `planSearch` could say `via: 'tool'` and the
    // run used to throw that half away, posting a bare completion at a model
    // chosen precisely because it CANNOT search. The supplier resolves once at
    // `begin` and rides the checkpoint, so every query of every round takes the
    // same path — including the queries a reclaim re-runs.
    const w = makeWorld({
      searchPlanFor: async () => ({ model: 'deepseek/deepseek-v4-flash', via: 'tool' as const, supplier: { server: 'talaria', tool: 'web_search' } }),
    })
    const out = await driveRun(w, { ...INPUT, mode: 'expedition' })
    expect(out.stop).toBe('done')
    expect(w.suppliers).toHaveLength(12)
    expect(new Set(w.suppliers.map((s) => `${s?.server}.${s?.tool}`))).toEqual(new Set(['talaria.web_search']))
  })

  it('stops early when the persona says the question is saturated', async () => {
    const w = makeWorld({ planQueries: async () => [] })
    const out = await driveRun(w, { ...INPUT, mode: 'expedition' })
    // No sources and nothing more to search — the point here is that an empty
    // plan ends the ROUND LOOP instead of spinning it. The run then retries
    // the same empty round its two times (plan → synthesize, three pairs) and
    // ends on the no-sources sentence: 7 steps, not the 2 a straight error
    // would take.
    expect(out.stop).toBe('error')
    expect(out.error).toBe('no sources found. Search returned nothing citable.')
    expect(w.searched).toHaveLength(0)
    expect(out.steps).toBe(7)
  })
})

// ── Resume ───────────────────────────────────────────────────────────────────

describe('resuming', () => {
  it('re-enters mid-round without re-running a completed query', async () => {
    const w = makeWorld()
    // Step 1 is `begin`, 2 is `plan`, 3/4/5 are the three searches. Killing the
    // checkpoint AFTER the second search is the deploy landing mid-round.
    const out = await driveRun(w, INPUT, { killAt: 4 })
    expect(out.stop).toBe('done')
    // Three distinct angles, and the ONE re-billed call is the query that was
    // in flight when the process died — not the round, and not the run.
    expect(w.searched).toEqual(['angle 1.1', 'angle 1.2', 'angle 1.2', 'angle 1.3'])
    expect(w.planned).toBe(1)
    // And the report still cites every source exactly once: the registry is
    // rebuilt from the checkpoint, so the resumed run does not renumber.
    expect(w.finished[0]?.stats.sources).toBe(3)
  })

  it('does not mark a restart as an error — anywhere', async () => {
    const w = makeWorld()
    for (const killAt of [1, 2, 3, 4, 5, 6, 7, 8]) {
      const world = makeWorld()
      const out = await driveRun(world, INPUT, { killAt })
      expect(out.stop, `killed after step ${killAt}`).toBe('done')
      // The sentence this whole port exists to delete has no path left to
      // reach: nothing wrote a failure on the research record, and the run
      // finished with a report.
      expect(world.failed, `killed after step ${killAt}`).toEqual([])
      expect(world.finished).toHaveLength(1)
    }
    expect(w.failed).toEqual([])
  })

  it('keeps the findings it had: a resumed run re-searches one query, not the run', async () => {
    const w = makeWorld()
    await driveRun(w, { ...INPUT, mode: 'expedition' }, { killAt: 9 })
    // Twelve queries plus exactly one repeat.
    expect(w.searched).toHaveLength(13)
  })
})

// ── The artifact ─────────────────────────────────────────────────────────────

describe('the report artifact', () => {
  // A recon run's steps, in order: 1 begin, 2 search, 3 plan (the round is
  // over, so this one only advances it), 4 synthesize, 5 artifact, 6 save,
  // 7 publish.

  it('a killed synthesis does not produce two artifacts', async () => {
    const w = makeWorld()
    const out = await driveRun(w, { ...INPUT, mode: 'recon' }, { killAt: 4 })
    expect(out.stop).toBe('done')
    expect(w.synthesized).toBe(2) // re-billed, which is the cost this port declares
    expect(w.created).toHaveLength(1)
    expect(new Set(w.written.map((x) => x.artifactId)).size).toBe(1)
    expect(w.finished[0]?.artifactId).toBe('art-1')
  })

  it('a crash between creating the artifact and checkpointing it still writes one report', async () => {
    // THE window: the artifact exists and no checkpoint says so. The link is
    // what the next entry finds it by.
    const w = makeWorld()
    const out = await driveRun(w, { ...INPUT, mode: 'recon' }, { killAt: 5 })
    expect(out.stop).toBe('done')
    expect(w.created).toHaveLength(1)
    expect(w.written).toHaveLength(1)
    expect(w.written[0]?.artifactId).toBe('art-1')
  })

  it('reuses the artifact a previous entry linked, even with no checkpoint at all', async () => {
    // The worst version: the process died between `createArtifact` and the
    // link's own statement never mattered because the link IS how the next
    // entry finds it. A run resumed against a link it did not write reuses it.
    const w = makeWorld()
    w.link = 'art-from-a-dead-driver'
    const out = await driveRun(w, { ...INPUT, mode: 'recon' })
    expect(out.stop).toBe('done')
    expect(w.created).toHaveLength(0)
    expect(w.written[0]?.artifactId).toBe('art-from-a-dead-driver')
  })

  it('re-writes the same body rather than a second report when the save is repeated', async () => {
    const w = makeWorld()
    await driveRun(w, { ...INPUT, mode: 'recon' }, { killAt: 6 })
    expect(w.created).toHaveLength(1)
    expect(w.written).toHaveLength(2)
    expect(w.written[0]?.body).toBe(w.written[1]?.body)
    expect(w.finished).toHaveLength(2)
    expect(w.finished[0]?.artifactId).toBe(w.finished[1]?.artifactId)
  })

  it('notifies once on a clean run, and the bell is the last thing it does', async () => {
    const w = makeWorld()
    await driveRun(w, { ...INPUT, mode: 'recon' })
    expect(w.indexed).toBe(1)
    expect(w.notified).toBe(1)
  })
})

// ── Nothing citable: the run answers itself ──────────────────────────────────
// This suite was rewritten when the park was removed (2026-08-28, ticket #5:
// two runs sat 'awaiting' for hours reading as working ones). The harness
// retries by itself and then fails; nobody is ever asked.

describe('when nothing citable comes back', () => {
  it('retries by itself — nobody is asked, the run never parks', async () => {
    const w = makeWorld()
    w.allSearchesDead = true
    const out = await driveRun(w, { ...INPUT, mode: 'recon' })
    // One initial pass plus two retries — MAX_NO_SOURCE_RETRIES, then the end.
    expect(out.stop).toBe('error')
    expect(out.error).toBe('no sources found. Search returned nothing citable.')
    // A retried RECON re-searches its one query — it does not quietly acquire
    // the planning stage its mode does not have.
    expect(w.searched).toEqual([INPUT.question, INPUT.question, INPUT.question])
    expect(w.planned).toBe(0)
    // And the domain record carries the failure, so the question can be
    // asked again.
    expect(w.failed).toEqual(['no sources found. Search returned nothing citable.'])
  })

  it('a transient outage answers itself — the retry round completes the run', async () => {
    const w = makeWorld()
    w.allSearchesDead = true
    w.reviveAfter = 1 // dead for the first search only
    const out = await driveRun(w, { ...INPUT, mode: 'recon' })
    expect(out.stop).toBe('done')
    expect(w.searched).toEqual([INPUT.question, INPUT.question])
    expect(w.finished[0]?.stats.sources).toBe(1)
    expect(w.written[0]?.body).toContain('A report')
  })

  it('one dead query costs one angle, not the run', async () => {
    const w = makeWorld()
    w.deadQueries.add('angle 1.2')
    const out = await driveRun(w, INPUT)
    expect(out.stop).toBe('done')
    expect(w.finished[0]?.stats.sources).toBe(2)
    expect(w.written[0]?.body).toContain('A report')
  })
})

// ── The failure mirror, and who may decide ───────────────────────────────────

describe('failures and audience', () => {
  it('puts a failed synthesis on the research record, and writes no artifact', async () => {
    const w = makeWorld({
      synthesize: async () => {
        throw new Error('harness "research-synthesis" could not reach "nomad": persona gateway 502')
      },
    })
    const out = await driveRun(w, { ...INPUT, mode: 'recon' })
    expect(out.stop).toBe('error')
    expect(w.created).toEqual([])
    expect(w.written).toEqual([])
    expect(w.failed[0]).toContain('persona gateway 502')
  })

  it('stops when the research record has been deleted', async () => {
    const w = makeWorld()
    const def = makeResearchRun(w.deps)
    w.rowExists = false
    const res = await def.step({
      run: rowFor(INPUT),
      input: INPUT,
      checkpoint: { stage: 'plan', searchModel: 'sonar', rounds: 1, perRound: 3, round: 1, plan: [], done: 0, queriesRun: 0, findings: [], sources: [], searchFailed: false, retries: 0, report: null, artifactId: null },
      decision: null,
      signal: new AbortController().signal,
      log: () => {},
      attempt: 0,
    })
    expect(res.kind).toBe('done')
    expect(w.planned).toBe(0)
  })

  it('asks the OWNER, and the admins only for a run nobody owns', () => {
    const def = makeResearchRun(makeWorld().deps)
    expect(def.audience(rowFor(INPUT))).toEqual({ by: 'user', userIds: ['user-1'] })
    expect(def.audience(rowFor({ ...INPUT, ownerUserId: null }, { ownerUserId: null }))).toEqual({ by: 'admin' })
  })

  it('does not record an abandoned step as a research failure', async () => {
    // A lost lease aborts the step; the run is not failed, another instance
    // resumes it. Writing `error` on the research record there would put a
    // failure on a run that is still working.
    const ac = new AbortController()
    const w = makeWorld({
      search: async () => {
        ac.abort()
        throw new Error('aborted')
      },
    })
    const def = makeResearchRun(w.deps)
    await expect(
      def.step({
        run: rowFor(INPUT),
        input: INPUT,
        checkpoint: { stage: 'search', searchModel: 'sonar', rounds: 1, perRound: 1, round: 1, plan: ['q'], done: 0, queriesRun: 0, findings: [], sources: [], searchFailed: false, retries: 0, report: null, artifactId: null },
        decision: null,
        signal: ac.signal,
        log: () => {},
        attempt: 0,
      }),
    ).rejects.toThrow()
    expect(w.failed).toEqual([])
  })
})

// ── The registry survives the trip through the checkpoint ────────────────────

describe('the citation registry', () => {
  it('rehydrates with its numbering intact, so a resumed run does not renumber', () => {
    const first = new SourceRegistry()
    first.add({ url: 'https://a', title: 'A', snippet: null })
    first.add({ url: 'https://b', title: 'B', snippet: null })
    const resumed = SourceRegistry.from(first.list())
    expect(resumed.add({ url: 'https://a', title: 'A', snippet: null })).toBe(1)
    expect(resumed.add({ url: 'https://c', title: 'C', snippet: null })).toBe(3)
    expect(resumed.size).toBe(3)
  })

  it('rebuilds in idx order however the checkpoint stored it', () => {
    const scrambled: ResearchSource[] = [
      { idx: 2, url: 'https://b', title: 'B', snippet: null },
      { idx: 1, url: 'https://a', title: 'A', snippet: null },
    ]
    const reg = SourceRegistry.from(scrambled)
    expect(reg.add({ url: 'https://a', title: null, snippet: null })).toBe(1)
    expect(reg.add({ url: 'https://b', title: null, snippet: null })).toBe(2)
  })

  it('continues above the highest when the parent seeded a gap, not at size + 1', () => {
    // THE DIVERGENCE THIS CONSOLIDATION CLOSED. The registry this file used to
    // carry allocated `size + 1` and defended it with "the map is the whole
    // registry" — true only while nothing upstream ever wrote a gap. A parent
    // whose source list lost [2] (size 2, highest [3]) handed [3] to the
    // follow-up's first NEW source, re-aiming every citation the parent's
    // already-published prose makes to [3].
    const parent: ResearchSource[] = [
      { idx: 1, url: 'https://a', title: 'A', snippet: null },
      { idx: 3, url: 'https://c', title: 'C', snippet: null },
    ]
    const reg = SourceRegistry.from(parent)
    expect(reg.add({ url: 'https://d', title: 'D', snippet: null })).toBe(4)
    expect(reg.list().map((s) => s.idx).sort((a, b) => a - b)).toEqual([1, 3, 4])
  })
})

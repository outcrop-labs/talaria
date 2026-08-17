import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import {
  caseKey,
  IDLE_STATUS,
  metaOf,
  runEvalSweep,
  scoreHarness,
  turnsPerCase,
  inFlightFor,
  stopEvalSweep,
  type EvalCaseScore,
  type EvalSweepStatus,
  type HarnessMeta,
} from '@/server/fitness/evals'
import { MAX_TURNS } from '@/server/fitness/toolbox/dry-run'
import { defineHarness, type HarnessDefinition } from '@/server/harness/define'
import type { HarnessDeps, TransportReply, TransportRequest } from '@/server/harness/run'
import { builtinActivityHarnesses, type HarnessSource, type RegisteredHarness } from '@/server/harness/registry'
import type { GuardConfig } from '@/server/guardrails'

// The sweep is exercised END TO END THROUGH THE REAL `runHarness`, against
// recorded replies. That is the whole point of the file: the number this suite
// prints has to be the number `harness_runs.schema_valid` carries, and a test
// that stubbed the runner would be asserting about a stub instead. Only the
// edges are faked — the registry, the transport, and the settings row the
// status lives in.
//
// Everything else is real: the parser, the zod schemas, the `verify` hook, the
// repair loop, the failure policies, and the row the runner hands to
// `recordRun`.

// ── A fake registry ──────────────────────────────────────────────────────────

/** `registry.ts` keeps its own `register` private, so the shape is rebuilt here
 *  — including `use`, which is the only way to apply a generic function to a
 *  definition with its I and O still paired. */
function reg<I, O>(def: HarnessDefinition<I, O>, source: HarnessSource = 'builtin'): RegisteredHarness {
  return {
    id: def.id,
    label: def.label,
    job: def.job,
    source,
    requires: def.requires,
    floor: def.floor,
    model: def.model,
    outputKind: def.output.kind,
    tools: def.tools ?? 'none',
    bandOf: Object.fromEntries((def.evals ?? []).map((e) => [e.name, e.band ?? ('standard' as const)])),
    widen: def.widen ?? null,
    guard: def.guard ?? null,
    temperature: def.temperature ?? null,
    evalNames: (def.evals ?? []).map((e) => e.name),
    use: (fn) => fn(def),
  }
}

const FLOOR = { capabilities: [], refuseBelow: false, note: 'Runs on anything.' }

const PICK = z.object({ pick: z.string() })
type Pick = z.infer<typeof PICK>

/** A JSON harness with the contract's input-relational half on `verify` — the
 *  blurb-writer shape, which is the case where a schema alone lies. */
const picker = (id: string, cases: Array<{ name: string; want: string; check?: (v: Pick) => string | null }>) =>
  defineHarness<{ want: string }, Pick>({
    id,
    label: 'Picker',
    job: 'Echoes back the id it was given.',
    requires: ['json'],
    floor: FLOOR,
    model: { chain: [] },
    render: (input) => [{ role: 'user', content: `pick ${input.want}` }],
    output: {
      kind: 'json',
      schema: PICK,
      // The schema cannot see the input, so only this can say "the id you sent
      // back is the id I asked for".
      verify: (value, input) => (value.pick === input.want ? null : `the pick must be '${input.want}', not '${value.pick}'`),
    },
    onFailure: 'null',
    evals: cases.map((c) => ({
      name: c.name,
      input: { want: c.want },
      check: c.check ?? ((v: Pick) => (v.pick === c.want ? null : `expected '${c.want}'`)),
    })),
  })

// ── The fake world ───────────────────────────────────────────────────────────

interface World {
  /** Replies per model call, in order. The last one repeats, exactly as
   *  `run.test.ts` does it. */
  replies: Array<string | Promise<never>>
  guardMode?: GuardConfig['mode']
  /** Called with the 1-based call number, before the reply is handed back. */
  onCall?: (n: number) => void
  /** Does the candidate's transport run the model's own tool loop? Defaults to
   *  true (a fleet persona), which is the answer that changes nothing. */
  ownTools?: boolean
  /** Can it be handed tool DEFINITIONS instead — the gateway's answer, and what
   *  lets the sweep supply the loop itself and dry-run against the sandbox. */
  toolDefs?: boolean
  /** Tool calls to emit alongside `replies[n]`, so a dry run can be driven
   *  exactly. Anything past the end emits none, which ends the loop. */
  toolCalls?: Array<Array<{ name: string; args: string }> | undefined>
}

interface Bench {
  status: EvalSweepStatus
  calls: TransportRequest[]
  deps: {
    harnesses: () => Promise<RegisteredHarness[]>
    harnessDeps: Partial<HarnessDeps>
    readStatus: () => Promise<EvalSweepStatus>
    writeStatus: (s: EvalSweepStatus) => Promise<void>
    price: (model: string, p: number, c: number) => Promise<number | null>
    servesOwnTools: (model: string) => Promise<boolean>
    acceptsToolDefinitions: (model: string) => Promise<boolean>
    now: () => number
  }
}

const CONFIG: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

function bench(harnesses: RegisteredHarness[], w: World): Bench {
  const calls: TransportRequest[] = []
  let clock = 1_000
  const state = { status: { ...IDLE_STATUS } as EvalSweepStatus }

  const harnessDeps: Partial<HarnessDeps> = {
    transport: async (req): Promise<TransportReply> => {
      calls.push(req)
      w.onCall?.(calls.length)
      const reply = w.replies[Math.min(calls.length - 1, w.replies.length - 1)] ?? ''
      // A promise that never settles is how a hanging harness is spelled: the
      // signal is fired at it and nothing honors the signal, which is the case
      // the bound exists for.
      if (typeof reply !== 'string') return reply
      const emitted = w.toolCalls?.[calls.length - 1]
      return {
        kind: 'gateway',
        text: reply,
        toolNames: [],
        ...(emitted ? { toolCalls: emitted } : {}),
        usage: { promptTokens: 40, completionTokens: 10 },
        contractDropped: false,
      }
    },
    // The model is pinned by the sweep, so resolution never runs; the rest are
    // the edges that would otherwise reach a database.
    routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
    personaKeys: async () => [],
    missingCapabilities: async () => [],
    capabilities: async () => ({}),
    guardConfig: async () => ({ ...CONFIG, mode: w.guardMode ?? 'observe' }),
    guardText: async () => [],
    now: () => (clock += 25),
  }

  const b: Bench = {
    get status() {
      return state.status
    },
    calls,
    deps: {
      harnesses: async () => harnesses,
      harnessDeps,
      readStatus: async () => state.status,
      writeStatus: async (s) => {
        // Round-tripped through JSON the way `app_settings` stores it, so a
        // value that would not survive the settings row cannot pass here.
        state.status = JSON.parse(JSON.stringify(s)) as EvalSweepStatus
      },
      price: async (_model, p, c) => (p + c) / 1_000_000,
      servesOwnTools: async () => w.ownTools ?? true,
      acceptsToolDefinitions: async () => w.toolDefs ?? false,
      now: () => 1_700_000_000_000,
    },
  }
  return b
}

const obj = (pick: string): string => JSON.stringify({ pick })

// ── Scoring ──────────────────────────────────────────────────────────────────

const META: HarnessMeta = { id: 'h', label: 'H', source: 'builtin', outputKind: 'json', tools: 'none', requires: ['json'], verifies: true, repairable: true }

/** A recorded case, defaulted to a clean pass so each test states only the axis
 *  it is about. */
const score = (over: Partial<EvalCaseScore>): EvalCaseScore => ({
  harness: 'h',
  case: 'c',
  band: 'standard',
  skipped: null,
  contractHeld: true,
  firstPass: true,
  repairs: 0,
  answered: true,
  task: 'pass',
  taskError: null,
  gap: null,
  findings: 0,
  latencyMs: 10,
  promptTokens: 0,
  completionTokens: 0,
  costUsd: null,
  estimated: false,
  timedOut: false,
  optimistic: false,
  error: null,
  prompt: null,
  raw: null,
  turns: null,
  calls: null,
  upstream: null,
  startedAt: '2026-08-01T00:00:00.000Z',
  wallMs: 0,
  ...over,
})

/** Twenty cases at the given first-pass and after-repair rates. */
function population(firstPass: number, held: number): EvalCaseScore[] {
  const out: EvalCaseScore[] = []
  for (let i = 0; i < 20; i++) {
    const first = i < firstPass
    const ok = i < held
    out.push(score({ case: `c${i}`, firstPass: first, contractHeld: ok, repairs: first ? 0 : 1, task: ok ? 'pass' : 'unscored' }))
  }
  return out
}

describe('scoreHarness', () => {
  it('separates a model that repairs from one that does not — the 40/95 vs 40/45 call', () => {
    // THE NUMBER THE WHOLE FEATURE EXISTS FOR. Both models are 40% first-pass.
    // One is usable behind the repair turn and one is not, and before this
    // existed nothing in Talaria could tell them apart.
    const usable = scoreHarness(META, population(8, 19))
    const not = scoreHarness(META, population(8, 9))

    expect(usable.contractRate).toBeCloseTo(0.4)
    expect(not.contractRate).toBeCloseTo(0.4)
    expect(usable.repairRate).toBeCloseTo(0.95)
    expect(not.repairRate).toBeCloseTo(0.45)

    // The conditional reading of the same fact: the repair turn rescued
    // essentially everything for one and almost nothing for the other.
    expect(usable.repairYield).toBeCloseTo(11 / 12)
    expect(not.repairYield).toBeCloseTo(1 / 12)
  })

  it('does not print a rescue rate for a repair turn that cannot happen', () => {
    // Thirteen of the registry's harnesses are text, and `run.ts` sets
    // maxRepairs to 0 for every one of them. Reporting `repairYield: 0` there
    // would say "the repair turn rescued nothing" about a round-trip that was
    // never sent.
    const text: HarnessMeta = { ...META, outputKind: 'text', repairable: false }
    const s = scoreHarness(text, population(8, 8))
    expect(s.repairYield).toBeNull()
    expect(s.repairRate).toBeCloseTo(0.4)
    expect(s.contractRate).toBeCloseTo(0.4)

    // Zero on a harness that CAN repair is a real and much worse fact, and the
    // two must not print the same.
    expect(scoreHarness(META, population(8, 8)).repairYield).toBe(0)
  })

  it('is cumulative, so repairRate can never read below contractRate', () => {
    const s = scoreHarness(META, population(20, 20))
    expect(s.contractRate).toBe(1)
    expect(s.repairRate).toBe(1)
    expect(s.repairYield).toBeNull()
  })

  it('leaves taskScore null when nothing was task-scorable rather than printing zero', () => {
    const s = scoreHarness(META, [score({ contractHeld: false, firstPass: false, task: 'unscored' })])
    expect(s.taskScore).toBeNull()
    expect(s.repairRate).toBe(0)
  })

  it('averages guard findings per run and counts timeouts out of the latency sample', () => {
    const s = scoreHarness(META, [
      score({ case: 'a', findings: 2, latencyMs: 10 }),
      score({ case: 'b', findings: 0, latencyMs: 90 }),
      score({ case: 'c', timedOut: true, latencyMs: 0, contractHeld: false, firstPass: false, task: 'unscored' }),
    ])
    // Two findings over the two cases that produced a reply. A timed-out case
    // never reached the guard pass, so leaving it in the denominator would
    // dilute the rate by the share of cases that ran out of clock.
    expect(s.guardRate).toBe(1)
    expect(s.timeouts).toBe(1)
    expect(s.scored).toBe(2)
    // THE RATES DIVIDE BY `scored`, NOT BY `cases`. A timeout observed nothing
    // about the model's contract — the clock ran out, which is a fact about our
    // budget and the provider's latency. Counting it as a contract failure is
    // the mistake `skipped` was introduced to fix, wearing different clothes:
    // it capped research-search at 0.78 for any model however good.
    expect(s.cases).toBe(3)
    expect(s.contractRate).toBe(1)
    expect(s.repairRate).toBe(1)
    // Nearest-rank over the two cases that actually measured something.
    expect(s.latencyP50).toBe(10)
    expect(s.latencyP95).toBe(90)
  })
})

// ── The sweep ────────────────────────────────────────────────────────────────

describe('runEvalSweep', () => {
  it('scores the contract from the row the runner writes, not from a second predicate', async () => {
    // The reply parses against the schema and FAILS `verify` — the exact shape
    // of the blurb-writer bug, where the schema said yes and the caller threw
    // the value away. The runner records schema_valid false for it; so does
    // this suite, because it reads that row rather than re-deciding.
    const h = picker('picker', [{ name: 'echoes the id', want: 'qwen3-14b' }])
    const b = bench([reg(h)], { replies: [obj('Qwen3 14B'), obj('Qwen3 14B')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    const one = sweep.cases[0]

    expect(one?.contractHeld).toBe(false)
    expect(one?.firstPass).toBe(false)
    // The runner's own repair turn fired and the model repeated itself.
    expect(one?.repairs).toBe(1)
    expect(one?.answered).toBe(true)
    // No model value survived, so the fixture graded nothing — a contract
    // failure must not be charged twice.
    expect(one?.task).toBe('unscored')
    expect(one?.error).toContain("the pick must be 'qwen3-14b'")
    // The drill-down keeps the prompt and the reply for a case that failed.
    expect(one?.prompt).toContain('pick qwen3-14b')
    expect(one?.raw).toContain('Qwen3 14B')

    expect(sweep.harnesses[0]?.contractRate).toBe(0)
    expect(sweep.harnesses[0]?.repairRate).toBe(0)
    expect(sweep.state).toBe('done')
  })

  it('counts a reply that only the repair turn fixed as repaired, not as first-pass', async () => {
    const h = picker('picker', [{ name: 'echoes the id', want: 'a' }])
    const b = bench([reg(h)], { replies: [obj('A'), obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    const one = sweep.cases[0]

    expect(one?.contractHeld).toBe(true)
    expect(one?.firstPass).toBe(false)
    expect(one?.repairs).toBe(1)
    expect(one?.task).toBe('pass')
    expect(sweep.harnesses[0]?.contractRate).toBe(0)
    expect(sweep.harnesses[0]?.repairRate).toBe(1)
    expect(sweep.harnesses[0]?.repairYield).toBe(1)
  })

  it('flags a value the contract accepted and the fixture rejected', async () => {
    // The contract holds (the id came back verbatim) and the fixture asserts
    // something further — quality the harness deliberately does not police.
    // That is `optimistic`: expected here, a bug where the assertion is one the
    // caller depends on.
    const h = picker('picker', [{ name: 'short answers only', want: 'a', check: () => 'the answer is not short enough' }])
    const b = bench([reg(h)], { replies: [obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    expect(sweep.cases[0]?.contractHeld).toBe(true)
    expect(sweep.cases[0]?.task).toBe('fail')
    expect(sweep.cases[0]?.optimistic).toBe(true)
    expect(sweep.harnesses[0]?.optimistic).toBe(1)
    expect(sweep.harnesses[0]?.taskScore).toBe(0)
    expect(sweep.harnesses[0]?.contractRate).toBe(1)
  })

  it('does not award task points for a declared fallback the model never produced', async () => {
    const fallback = defineHarness<{ q: string }, Pick>({
      id: 'fallback',
      label: 'Fallback',
      job: 'Has a declared safe answer.',
      requires: ['json'],
      floor: FLOOR,
      model: { chain: [] },
      render: () => [{ role: 'user', content: 'go' }],
      output: { kind: 'json', schema: PICK },
      onFailure: { fallback: { pick: 'safe' } },
      evals: [{ name: 'holds the shape', input: { q: 'x' }, check: (v) => (v.pick === 'safe' ? null : 'wrong') }],
    })
    const b = bench([reg(fallback)], { replies: ['not json at all', 'still not json'] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    // The runner hands the fallback back with schemaValid false. Grading it
    // would score the harness author's constant as a model win.
    expect(sweep.cases[0]?.contractHeld).toBe(false)
    expect(sweep.cases[0]?.task).toBe('unscored')
    expect(sweep.harnesses[0]?.taskScore).toBeNull()
  })

  it('records the guard findings the run row counted, and says when the guard was off', async () => {
    const h = picker('picker', [{ name: 'leaks', want: 'a' }])
    // `secret_leak` over a live-looking key in the reply. The guard pass is the
    // real rule registry — this suite fakes the transport, never the rules.
    const leak = JSON.stringify({ pick: 'a', note: 'use sk-live-9f4c2a7b1e6d80541122334455667788' })
    const b = bench([reg(h)], { replies: [leak] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    expect(sweep.guarded).toBe(true)
    expect(sweep.cases[0]?.findings).toBeGreaterThan(0)
    expect(sweep.harnesses[0]?.guardRate).toBeGreaterThan(0)

    const off = bench([reg(h)], { replies: [leak], guardMode: 'off' })
    const quiet = await runEvalSweep('candidate', { deps: off.deps })
    // Zero findings, and the sweep says WHY — zero-because-off must not read as
    // zero-because-clean.
    expect(quiet.guarded).toBe(false)
    expect(quiet.cases[0]?.findings).toBe(0)
  })

  it('bounds a hanging harness and keeps going', async () => {
    const stuck = picker('stuck', [{ name: 'never answers', want: 'a' }])
    const fine = picker('fine', [{ name: 'answers', want: 'a' }])
    // The first call never settles and never rejects: a persona container that
    // accepted the connection and went away. The abort signal is fired at it
    // and this transport, like several real ones, does not honor it.
    const hang: Promise<never> = new Promise(() => {})
    let n = 0
    const b = bench([reg(stuck), reg(fine)], {
      replies: [hang, obj('a')],
      onCall: () => {
        n++
      },
    })

    // `caseTimeoutMs` is the PER-TURN allowance now, not the whole case: a case
    // is not one model call (a dry run takes up to `MAX_TURNS`, a JSON harness
    // can add a repair), and a flat budget applied to a multi-call case timed
    // out the harnesses that call the most and charged it to the model. These
    // fixtures are JSON with one repair turn, so 30ms per turn is 60ms a case.
    const sweep = await runEvalSweep('candidate', { deps: b.deps, caseTimeoutMs: 30, pressureBackoffMs: [1] })

    // THREE CALLS, NOT TWO: the first request is lost, so it is asked ONCE more
    // (`TIMEOUT_RETRIES`) before the sweep gives up on it. Here the second
    // attempt answers — which is the whole argument for the retry, since the
    // first attempt measured nothing about the model at all.
    expect(n).toBe(3)
    const stuckCase = sweep.cases.find((c) => c.harness === 'stuck')
    expect(stuckCase?.timedOut).toBe(false)
    expect(stuckCase?.contractHeld).toBe(true)
    // The sweep did not strand: the next harness ran and scored.
    expect(sweep.cases.find((c) => c.harness === 'fine')?.contractHeld).toBe(true)
    expect(sweep.state).toBe('done')
    expect(sweep.done).toBe(2)
  })

  it('does not let a throwing harness end the sweep', async () => {
    const thrower = defineHarness<{ q: string }, Pick>({
      id: 'thrower',
      label: 'Thrower',
      job: 'Declares onFailure: throw.',
      requires: ['json'],
      floor: FLOOR,
      model: { chain: [] },
      render: () => [{ role: 'user', content: 'go' }],
      output: { kind: 'json', schema: PICK },
      onFailure: 'throw',
      evals: [{ name: 'holds the shape', input: { q: 'x' }, check: () => null }],
    })
    const after = picker('after', [{ name: 'answers', want: 'a' }])
    const b = bench([reg(thrower), reg(after)], { replies: ['nope', 'nope', obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    const failed = sweep.cases.find((c) => c.harness === 'thrower')
    expect(failed?.contractHeld).toBe(false)
    // The row is written before the throw, so the failure stays visible with
    // its repair count and its sentence intact.
    expect(failed?.repairs).toBe(1)
    expect(failed?.error).toContain('thrower')
    expect(sweep.cases.find((c) => c.harness === 'after')?.contractHeld).toBe(true)
    expect(sweep.state).toBe('done')
  })

  it('survives a fixture check that throws', async () => {
    const h = picker('picker', [
      {
        name: 'a badly written assertion',
        want: 'a',
        check: () => {
          throw new Error('cannot read properties of undefined')
        },
      },
    ])
    const b = bench([reg(h)], { replies: [obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    expect(sweep.cases[0]?.contractHeld).toBe(true)
    expect(sweep.cases[0]?.task).toBe('fail')
    expect(sweep.cases[0]?.taskError).toContain('the fixture check threw')
    expect(sweep.state).toBe('done')
  })

  it('leaves consistent, resumable state when an admin stops it', async () => {
    const first = picker('first', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const second = picker('second', [{ name: 'three', want: 'a' }])
    const b = bench([reg(first), reg(second)], {
      replies: [obj('a')],
      // Stop after the first case has been answered, so this exercises the
      // BOUNDARY path. The mid-case path is the test below.
      onCall: (n) => {
        if (n === 1) stopEvalSweep()
      },
    })

    // WIDTH 1, because this test is about stop-and-resume semantics rather than
    // about concurrency: at the default width several cases are already in
    // flight when the stop lands, so "exactly one case was recorded" would be
    // asserting the pool's timing instead of the ledger's consistency.
    const stopped = await runEvalSweep('candidate', { deps: b.deps, concurrency: 1 })

    expect(stopped.state).toBe('stopped')
    // CONSISTENT: the ledger, the counter and the persisted row all agree, and
    // nothing was scored for a case that never ran.
    expect(stopped.cases).toHaveLength(1)
    expect(stopped.done).toBe(1)
    expect(stopped.total).toBe(3)
    expect(b.status.state).toBe('stopped')
    expect(b.status.done).toBe(1)
    expect(b.status.cases.map((c) => caseKey(c.harness, c.case))).toEqual(['first::one'])
    expect(b.status.finishedAt).not.toBeNull()
    expect(b.calls).toHaveLength(1)

    // RESUMABLE: the same candidate picks up where it left off and does not
    // re-buy the case it already paid for.
    const resumed = await runEvalSweep('candidate', { deps: b.deps, concurrency: 1 })
    expect(resumed.state).toBe('done')
    expect(resumed.done).toBe(3)
    expect(resumed.cases.map((c) => caseKey(c.harness, c.case))).toEqual(['first::one', 'first::two', 'second::three'])
    expect(b.calls).toHaveLength(3)
  })

  it('does not resume across candidates', async () => {
    const h = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(h)], {
      replies: [obj('a')],
      onCall: (n) => {
        if (n === 1) stopEvalSweep()
      },
    })
    // Width 1 for the same reason as above: this is about whose ledger it is,
    // not about how many cases were in flight when the stop landed.
    await runEvalSweep('candidate-a', { deps: b.deps, concurrency: 1 })
    expect(b.status.cases).toHaveLength(1)

    // A different model discards the ledger: a matrix cell assembled from two
    // models is a number with no referent.
    const other = await runEvalSweep('candidate-b', { deps: b.deps })
    expect(other.model).toBe('candidate-b')
    expect(other.done).toBe(2)
    expect(other.cases.every((c) => c.contractHeld)).toBe(true)
  })

  it('records a tool-loop harness as skipped only when NOTHING can drive its loop', async () => {
    // THE DEFECT THIS LOCKS. `work-session`, `outreach:check-in` and
    // `briefer:chat` declare `tools: 'own'` because the tool loop IS the
    // feature. On an org-gateway model the transport refuses them in about four
    // milliseconds, before a token is spent — and the sweep used to record that
    // refusal as `contractHeld: false`, so the matrix printed "0% first pass"
    // for a model nothing had asked a question.
    //
    // A model that can be handed DEFINITIONS is dry-run instead (below); this is
    // the remaining case, where neither path exists and a skip is the honest
    // answer.
    const looper = { ...reg(picker('looper', [{ name: 'a', want: 'a' }, { name: 'b', want: 'b' }])), tools: 'own' as const }
    const plain = reg(picker('plain', [{ name: 'a', want: 'a' }]))
    const b = bench([looper, plain], { replies: [obj('a')], ownTools: false, toolDefs: false })

    const sweep = await runEvalSweep('gw/model', { deps: b.deps })

    // ONE call, for the one harness that could run. The skipped fixtures cost
    // nothing, which is half the point.
    expect(b.calls).toHaveLength(1)

    const skipped = sweep.cases.filter((c) => c.skipped !== null)
    expect(skipped).toHaveLength(2)
    expect(skipped[0]?.skipped).toContain('neither run its own nor be handed tool definitions')
    // No transcript on a case that never ran: there is nothing to drill into.
    expect(skipped[0]?.prompt).toBeNull()
    expect(skipped[0]?.error).toBeNull()

    const looped = sweep.harnesses.find((h) => h.id === 'looper')
    // `cases` is the RUN denominator. Zero of them ran, so every rate is the
    // n===0 zero and `skipped` carries the count — a consumer reading `cases`
    // sees "no evidence", which is the truth.
    expect(looped?.cases).toBe(0)
    expect(looped?.skipped).toBe(2)
    expect(looped?.skipReason).toContain('nothing here is a measurement of it')

    // The harness that CAN run is untouched by any of this.
    expect(sweep.harnesses.find((h) => h.id === 'plain')?.contractRate).toBe(1)
    // Progress still reaches its total, so the bar completes and a resume does
    // not re-enter the same skip.
    expect(sweep.done).toBe(sweep.total)
  })

  it('runs a tool-loop harness normally when the candidate is a fleet persona', async () => {
    const looper = { ...reg(picker('looper', [{ name: 'a', want: 'a' }])), tools: 'own' as const }
    const b = bench([looper], { replies: [obj('a')], ownTools: true })

    const sweep = await runEvalSweep('engineer-engineering', { deps: b.deps })

    expect(b.calls).toHaveLength(1)
    expect(sweep.cases[0]?.skipped).toBeNull()
    expect(sweep.harnesses[0]?.cases).toBe(1)
    expect(sweep.harnesses[0]?.skipped).toBe(0)
  })

  it('records the whole tool conversation on a dry run, and the calls even when it passes', async () => {
    // THE DRILL-DOWN'S RAW MATERIAL. A behavioural fixture's verdict is one
    // sentence, and that sentence is either about the model or about our
    // harness. Nothing on the page could tell those apart until the turns and
    // the calls were archived beside it.
    const looper = {
      ...reg(
        defineHarness<{ q: string }, string>({
          id: 'looper',
          label: 'Looper',
          job: 'Works a ticket.',
          requires: [],
          floor: FLOOR,
          model: { chain: [] },
          render: () => [{ role: 'user', content: 'work PLAT-118' }],
          output: { kind: 'text', clean: (raw) => raw.trim() },
          onFailure: 'null',
          tools: 'own',
          dryRun: { tools: ['get_ticket', 'comment'] },
          evals: [{ name: 'reads before it writes', input: { q: 'go' }, check: (_v, ctx) => (ctx.calledBefore('get_ticket', 'comment') ? null : 'wrote first') }],
        }),
      ),
    }
    const b = bench([looper], {
      replies: ['', 'Read it and acknowledged.'],
      ownTools: false,
      toolDefs: true,
      toolCalls: [
        [
          { name: 'get_ticket', args: JSON.stringify({ taskId: 'PLAT-118' }) },
          { name: 'comment', args: JSON.stringify({ taskId: 'PLAT-118', content: 'On it.' }) },
        ],
      ],
    })

    const sweep = await runEvalSweep('gw/model', { deps: b.deps })
    const c = sweep.cases[0]!

    expect(c.task).toBe('pass')
    // THE CALLS SURVIVE A PASS. Comparing two models on one fixture IS comparing
    // these two lists, so keeping them only on failure would hide the comparison
    // worth making.
    expect(c.calls?.map((x) => x.tool)).toEqual(['get_ticket', 'comment'])
    expect(JSON.parse(c.calls![1]!.args) as { content: string }).toMatchObject({ content: 'On it.' })
    // Results are dropped on a clean case — they are the whole weight, and there
    // is nothing to explain. The transcript goes for the same reason.
    expect(c.calls?.every((x) => x.result === null)).toBe(true)
    expect(c.turns).toBeNull()
  })

  it('keeps the turns and the tool results when a dry run FAILS its check', async () => {
    const looper = {
      ...reg(
        defineHarness<{ q: string }, string>({
          id: 'looper',
          label: 'Looper',
          job: 'Works a ticket.',
          requires: [],
          floor: FLOOR,
          model: { chain: [] },
          render: () => [{ role: 'user', content: 'work PLAT-118' }],
          output: { kind: 'text', clean: (raw) => raw.trim() },
          onFailure: 'null',
          tools: 'own',
          dryRun: { tools: ['get_ticket', 'comment'] },
          evals: [{ name: 'reads before it writes', input: { q: 'go' }, check: (_v, ctx) => (ctx.calledBefore('get_ticket', 'comment') ? null : 'commented on a ticket it had not read') }],
        }),
      ),
    }
    const b = bench([looper], {
      replies: ['', 'Commented.'],
      ownTools: false,
      toolDefs: true,
      toolCalls: [[{ name: 'comment', args: JSON.stringify({ taskId: 'PLAT-118', content: 'On it.' }) }]],
    })

    const sweep = await runEvalSweep('gw/model', { deps: b.deps })
    const c = sweep.cases[0]!

    expect(c.taskError).toBe('commented on a ticket it had not read')
    // The tool ANSWERED, and on a failure that answer is often the explanation.
    expect(c.calls?.[0]?.result).toContain('ok')
    // Assistant turn (carrying the call), tool turn (carrying the result), and
    // the closing question the loop asks a run that has stopped calling tools.
    expect(c.turns?.map((t) => t.role)).toEqual(['user', 'assistant', 'tool'])
    expect(c.turns?.find((t) => t.role === 'assistant')?.toolCalls).toEqual(['comment'])
  })

  it('stops a case ALREADY IN FLIGHT, and records nothing for it', async () => {
    // WHY THE BUTTON READ AS BROKEN. Stop used to be honored only between cases,
    // and a dry-run case is budgeted `PER_TURN_TIMEOUT_MS × turnsPerCase` —
    // minutes. The request landed immediately and the sweep politely finished a
    // case nobody wanted, so an admin saw nothing happen and pressed it again.
    const hang = new Promise<never>(() => {})
    const slow = picker('slow', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(slow)], {
      // The transport never settles AND never honors its signal — the worst
      // case, and the one that proves the sweep's response to Stop does not
      // depend on how well the thing underneath it behaves.
      replies: [hang],
      onCall: () => stopEvalSweep('candidate'),
    })

    const at = Date.now()
    const stopped = await runEvalSweep('candidate', { deps: b.deps, caseTimeoutMs: 600_000, concurrency: 1 })
    const took = Date.now() - at

    expect(stopped.state).toBe('stopped')
    // Under the case budget by three orders of magnitude. Before the fix this
    // would have sat here for the full ten minutes.
    expect(took).toBeLessThan(5_000)
    // CANCELLED IS NOT FAILED. Nothing is written for the case that was killed:
    // the persisted status is the resume ledger, so recording it would mark the
    // fixture done, skip it forever on resume, and leave the model carrying a
    // failure it was never given a chance at.
    expect(stopped.cases).toEqual([])
    expect(stopped.done).toBe(0)
    expect(b.status.cases).toEqual([])
    // It really was mid-flight: one call went out and no second one followed.
    expect(b.calls).toHaveLength(1)
  })

  it('honors a stop asked for by ANOTHER instance while a case is in flight', async () => {
    // `stopRequested` is in-process and empty in any worker that did not start
    // the run, so a cross-process Stop only ever arrives through `shouldStop`.
    // That used to be read once per HARNESS — eleven work-session fixtures at up
    // to seven minutes each before it was noticed.
    const hang = new Promise<never>(() => {})
    const slow = picker('slow', [{ name: 'one', want: 'a' }])
    const b = bench([reg(slow)], { replies: [hang] })

    // FALSE AT THE HARNESS BOUNDARY, TRUE AFTER — otherwise the sweep would
    // break before it ever started a case and this test would pass without
    // exercising the path it is about.
    let asks = 0
    const at = Date.now()
    const stopped = await runEvalSweep('candidate', {
      deps: b.deps,
      caseTimeoutMs: 600_000,
      // Not this process's flag: the persisted one, as `surface.ts` supplies it.
      shouldStop: async () => ++asks > 1,
    })

    // It really did start the case before the stop arrived.
    expect(b.calls).toHaveLength(1)
    expect(asks).toBeGreaterThan(1)
    expect(stopped.state).toBe('stopped')
    expect(Date.now() - at).toBeLessThan(5_000)
    expect(stopped.cases).toEqual([])
  })

  it('says WHAT a timed-out case was waiting on, not just that it timed out', async () => {
    // THREE ROUNDS OF "still getting timeouts" WENT PAST ON ONE SENTENCE. "The
    // case did not finish inside 60000ms" is the least useful true statement
    // available: it cannot tell a slow model from a request that never came back
    // from a case that spent its budget on retries from time that never reached
    // the provider at all. Those are four different bugs with four different
    // fixes, and the report distinguished none of them.
    const hang = new Promise<never>(() => {})
    const b = bench([reg(picker('slow', [{ name: 'one', want: 'a' }]))], { replies: [hang] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps, caseTimeoutMs: 60, pressureBackoffMs: [1] })
    const c = sweep.cases[0]!

    expect(c.timedOut).toBe(true)
    // The call went out and never came back — which is the whole diagnosis, and
    // it survives onto the recorded case even after the retry has been spent.
    expect(c.upstream).toHaveLength(1)
    expect(c.upstream?.[0]?.settled).toBe(false)
    expect(c.skipped).toContain('never answered')
  })

  it('publishes the case it is on, with the turns, and clears it when the case ends', async () => {
    // WHAT A WEDGED SWEEP LOOKS LIKE OTHERWISE: a still image. The completed-case
    // feed cannot show the case that is not completing, which is always the one
    // worth looking at.
    const seen: Array<{ harness: string; case: string; turn: number; roles: string[] }> = []
    const b = bench([reg(picker('slow', [{ name: 'one', want: 'a' }]))], {
      replies: [obj('a')],
      onCall: () => {
        for (const f of inFlightFor('candidate')) seen.push({ harness: f.harness, case: f.case, turn: f.turn, roles: f.turns.map((t) => t.role) })
      },
    })

    await runEvalSweep('candidate', { deps: b.deps })

    expect(seen).toHaveLength(1)
    expect(seen[0]).toMatchObject({ harness: 'slow', case: 'one', turn: 1 })
    // The request IS the conversation so far, so the turns are populated before
    // the reply lands — which is the whole point on a case that never replies.
    expect(seen[0]?.roles.length).toBeGreaterThan(0)

    // CLEARED. A "running now" that outlives its sweep makes a finished run look
    // wedged, which is the confusion this panel exists to remove.
    expect(inFlightFor('candidate')).toEqual([])
  })

  it('clears the in-flight case when a run is STOPPED mid-case', async () => {
    const hang = new Promise<never>(() => {})
    const b = bench([reg(picker('slow', [{ name: 'one', want: 'a' }]))], {
      replies: [hang],
      onCall: () => stopEvalSweep('candidate'),
    })
    await runEvalSweep('candidate', { deps: b.deps, caseTimeoutMs: 600_000 })
    expect(inFlightFor('candidate')).toEqual([])
  })

  it('runs a harness\'s fixtures in PARALLEL, bounded by the requested width', async () => {
    // A 247-fixture sweep one at a time is most of an hour. What the old
    // sequential rule was protecting is preserved elsewhere — the width is
    // recorded so latency stays interpretable, and the pressure valve below
    // handles the deployment that cannot take it.
    let inFlightNow = 0
    let peak = 0
    const many = picker(
      'wide',
      Array.from({ length: 8 }, (_, i) => ({ name: `case-${i}`, want: 'a' })),
    )
    const b = bench([reg(many)], {
      replies: [obj('a')],
      onCall: () => {
        inFlightNow++
        peak = Math.max(peak, inFlightNow)
        // Resolve on a later tick so overlap is observable at all.
        queueMicrotask(() => inFlightNow--)
      },
    })

    const sweep = await runEvalSweep('candidate', { deps: b.deps, concurrency: 3 })

    expect(sweep.done).toBe(8)
    expect(sweep.cases.every((c) => c.contractHeld)).toBe(true)
    // Bounded: never more than the width, and more than one.
    expect(peak).toBeGreaterThan(1)
    expect(peak).toBeLessThanOrEqual(3)
    expect(sweep.concurrency.requested).toBe(3)
  })

  it('records every case exactly once whatever order they finish in', async () => {
    // Completion order is not submission order under a pool, and the resume
    // ledger is a SET of case keys — so the thing to prove is that the ledger and
    // the counter still agree when the order is scrambled.
    const many = picker(
      'wide',
      Array.from({ length: 6 }, (_, i) => ({ name: `case-${i}`, want: 'a' })),
    )
    const b = bench([reg(many)], { replies: [obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps, concurrency: 4 })

    const keys = sweep.cases.map((c) => caseKey(c.harness, c.case))
    expect(new Set(keys).size).toBe(6)
    expect(sweep.done).toBe(6)
    expect(b.status.done).toBe(6)
  })

  it('NARROWS ITSELF when the provider answers with rate limits, and says it did', async () => {
    // THE HALF THE SEQUENTIAL RULE EXISTED FOR. A self-hosted model behind one
    // GPU answers a parallel sweep with 429s, and scoring those as contract
    // failures records a fact about the hardware as a fact about the model. So
    // the sweep backs off instead, and the report carries the reason.
    const many = picker(
      'wide',
      Array.from({ length: 8 }, (_, i) => ({ name: `case-${i}`, want: 'a' })),
    )
    let n = 0
    const b = bench([reg(many)], {
      replies: [obj('a')],
      onCall: () => {
        n++
      },
    })
    // The first few calls come back rate-limited; the rest are fine.
    const throttled = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          if (++n <= 2) throw new Error('gateway completion 429: too many requests')
          return b.deps.harnessDeps.transport!(req)
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: throttled, concurrency: 8 })

    expect(sweep.concurrency.requested).toBe(8)
    // `low`, not `ended`: the valve reopens now, so a short run that recovers can
    // finish back at the width it started from. What the narrowing left behind is
    // the floor it reached.
    expect(sweep.concurrency.low).toBeLessThan(8)
    expect(sweep.concurrency.narrowedBecause).toContain('429')
  })

  it('NARROWS INSIDE THE HARNESS THAT HIT THE PRESSURE, not at the next one', async () => {
    // THE BUG. `pool` read the width ONCE, to size its lane array, and then ran
    // those lanes to exhaustion — so narrowing did nothing until the next harness
    // built a new pool. A single-harness sweep (or the last harness of any sweep)
    // therefore ran at full width no matter how hard the provider pushed back,
    // which is the opposite of what the valve is for. Two comments inside the
    // loop asserted the correct behaviour; neither was true of the code.
    const many = picker(
      'wide',
      Array.from({ length: 40 }, (_, i) => ({ name: `case-${i}`, want: 'a' })),
    )
    const b = bench([reg(many)], { replies: [obj('a')] })

    let inFlight = 0
    let seq = 0
    const samples: Array<{ seq: number; inFlight: number }> = []
    const throttled = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          // The very first call is the only pressure. One lost minute, then a
          // deployment that behaves perfectly for the remaining thirty-nine.
          if (++seq === 1) throw new Error('gateway completion 429: too many requests')
          inFlight++
          samples.push({ seq, inFlight })
          try {
            await new Promise((r) => setTimeout(r, 5))
            return await b.deps.harnessDeps.transport!(req)
          } finally {
            inFlight--
          }
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: throttled, concurrency: 8, pressureBackoffMs: [1, 1, 1] })

    expect(sweep.concurrency.narrowedBecause).toContain('429')
    // The eight lanes launched together, so the opening burst legitimately
    // overlaps at full width — the claim is about what happens AFTER the valve
    // has been told. From call twelve on, four lanes are parked and stay parked
    // until the recovery streak earns them back one at a time.
    const late = samples.filter((x) => x.seq >= 12)
    expect(late.length).toBeGreaterThan(10)
    expect(Math.max(...late.map((x) => x.inFlight))).toBeLessThanOrEqual(8)
    expect(Math.min(...samples.filter((x) => x.seq >= 12 && x.seq <= 20).map((x) => x.inFlight))).toBeLessThan(8)
  })

  it('REOPENS the valve after a clean stretch, instead of crawling for the rest of the run', async () => {
    // WHAT THIS COST IN PRODUCTION. A 247-case sweep of deepseek-v4-flash asked
    // for width 4, met one lost request in its first minute, and ran the
    // remaining two hundred and forty cases strictly sequentially. The archive
    // said so plainly — `requested: 4, ended: 1` — and nobody was told. A lost
    // request is not a capacity ceiling; treating it as a permanent one turned a
    // transient blip into a fourfold slowdown for the rest of the battery.
    const many = picker(
      'wide',
      Array.from({ length: 40 }, (_, i) => ({ name: `case-${i}`, want: 'a' })),
    )
    const b = bench([reg(many)], { replies: [obj('a')] })

    let seq = 0
    const flaky = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          if (++seq === 1) throw new Error('gateway completion 429: too many requests')
          return b.deps.harnessDeps.transport!(req)
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: flaky, concurrency: 4, pressureBackoffMs: [1, 1, 1] })

    expect(sweep.concurrency.requested).toBe(4)
    // It DID narrow — the reason is recorded and the floor is below what was
    // asked for — and it climbed all the way back before the run ended.
    expect(sweep.concurrency.narrowedBecause).toContain('429')
    expect(sweep.concurrency.low).toBeLessThan(4)
    expect(sweep.concurrency.ended).toBe(4)
  })

  it('does NOT reopen while the provider is still pushing back', async () => {
    // The objection the one-way valve was built against, and the reason recovery
    // is five-clean-cases-then-one-lane rather than an immediate reset: a real
    // ceiling must be found and HELD. Here every third call is refused, so the
    // sweep never assembles a clean streak long enough to climb, and it must not
    // spend the run sawtoothing back into the same 429.
    const many = picker(
      'wide',
      Array.from({ length: 40 }, (_, i) => ({ name: `case-${i}`, want: 'a' })),
    )
    const b = bench([reg(many)], { replies: [obj('a')] })

    let seq = 0
    const busy = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          if (++seq % 3 === 0) throw new Error('gateway completion 429: too many requests')
          return b.deps.harnessDeps.transport!(req)
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: busy, concurrency: 8, pressureBackoffMs: [1, 1, 1] })

    expect(sweep.concurrency.narrowedBecause).toContain('429')
    expect(sweep.concurrency.ended).toBeLessThan(8)
  })

  it('RETRIES a rate-limited fixture instead of failing it', async () => {
    // A 429 is the provider saying "slower", not the model answering badly.
    // Scoring one as a contract failure is the same category error as scoring a
    // 401 as a model that cannot hold JSON — a fact about the deployment,
    // recorded permanently in a matrix somebody buys a model from.
    let call = 0
    const b = bench([reg(picker('picker', [{ name: 'one', want: 'a' }]))], { replies: [obj('a')] })
    const flaky = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          // Busy twice, then fine — the shape of a real rate limit clearing.
          if (++call <= 2) throw new Error('gateway completion 429: rate limit exceeded')
          return b.deps.harnessDeps.transport!(req)
        },
      },
    }

    // The production gaps are seconds; the path is what is under test, not the
    // clock, so they are injected.
    const sweep = await runEvalSweep('candidate', { deps: flaky, concurrency: 1, pressureBackoffMs: [1, 1, 1] })
    const c = sweep.cases[0]!

    expect(call).toBe(3)
    // The answer we eventually got is the one that counts.
    expect(c.skipped).toBeNull()
    expect(c.contractHeld).toBe(true)
    expect(c.task).toBe('pass')
  })

  it('records a case the provider NEVER let us ask as unmeasured, not as failed', async () => {
    const b = bench([reg(picker('picker', [{ name: 'one', want: 'a' }]))], { replies: [obj('a')] })
    const busy = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async () => {
          throw new Error('gateway completion 429: too many requests')
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: busy, concurrency: 1, pressureBackoffMs: [1, 1, 1] })
    const c = sweep.cases[0]!

    // UNMEASURED, and `skipped` is what excludes it from every rate. A red cell
    // here would mean "your provider was busy" and be read as "this model cannot
    // hold a contract".
    expect(c.skipped).toContain('rate limits on every attempt')
    expect(c.task).toBe('unscored')
    expect(sweep.harnesses[0]?.cases).toBe(0)
    expect(sweep.harnesses[0]?.skipped).toBe(1)
    // And it narrowed itself on the way, which is the other half of the answer.
    expect(sweep.concurrency.narrowedBecause).toContain('429')
  })

  it('archives EVERY case for audit, including the ones that passed', async () => {
    // THE POINT OF THE ARCHIVE. The settings-row report keeps a transcript only
    // when something failed, which cannot answer the question an audit actually
    // asks — "did the model do the work, or did our fixture accept something
    // weak?" That is only answerable from a PASSING transcript, and those were
    // exactly the ones being thrown away.
    const filed: Array<{ model: string; run: string; case: string; verdictPassed: boolean }> = []
    const h = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(h)], { replies: [obj('a')] })

    let pruned = 0
    const sweep = await runEvalSweep('candidate', {
      deps: b.deps,
      concurrency: 1,
      archiveCase: async (model, run, score) => {
        filed.push({ model, run, case: score.case, verdictPassed: score.task === 'pass' })
      },
      archivePrune: async () => {
        pruned++
      },
    })

    expect(sweep.cases.every((c) => c.task === 'pass')).toBe(true)
    // BOTH of them, and both passed — the case the old rule discarded.
    expect(filed.map((f) => f.case)).toEqual(['one', 'two'])
    expect(filed.every((f) => f.verdictPassed)).toBe(true)
    // One run identity for the whole sweep, so an auditor reads a run rather
    // than a pile of rows.
    expect(new Set(filed.map((f) => f.run)).size).toBe(1)
    expect(pruned).toBe(1)
  })

  it('files a resumed sweep under the ORIGINAL run, not a second half-run', async () => {
    const first = picker('first', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(first)], {
      replies: [obj('a')],
      onCall: (n) => {
        if (n === 1) stopEvalSweep()
      },
    })
    const runs: string[] = []
    const archiveCase = async (_m: string, run: string) => {
      runs.push(run)
    }

    await runEvalSweep('candidate', { deps: b.deps, concurrency: 1, archiveCase })
    await runEvalSweep('candidate', { deps: b.deps, concurrency: 1, archiveCase })

    // An audit of "that run" has to mean one thing. A resume that opened a second
    // run identity would split the evidence in half down the middle.
    expect(new Set(runs).size).toBe(1)
    expect(runs).toHaveLength(2)
  })

  it('re-runs ONLY the cases that left a hole, keeping the passes', async () => {
    // The middle setting between resume and restart, and the one an admin wants
    // after a bad run: resume has nothing pending (every case is recorded) and
    // restart re-buys two hundred and forty-two cases to re-ask five.
    const h = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'b' },
      { name: 'three', want: 'a' },
    ])
    // 'two' wants 'b' and every reply is 'a', so its CONTRACT breaks (`verify`
    // rejects a pick that is not the one asked for) and the other two pass.
    const b = bench([reg(h)], { replies: [obj('a')] })
    const first = await runEvalSweep('candidate', { deps: b.deps, concurrency: 1 })
    expect(first.cases.filter((c) => !c.contractHeld).map((c) => c.case)).toEqual(['two'])
    const spent = b.calls.length

    const retried = await runEvalSweep('candidate', { deps: b.deps, concurrency: 1, retryFailed: true })

    // ONE CASE re-asked, not three. It costs more than one call because a JSON
    // harness gets a repair turn on a contract that did not hold — the point is
    // that `one` and `three` cost nothing at all.
    const reAsked = b.calls.length - spent
    expect(reAsked).toBeGreaterThan(0)
    expect(reAsked).toBeLessThan(spent)
    // The ledger is still whole — all three cases, none duplicated.
    expect(retried.cases.map((c) => c.case).sort()).toEqual(['one', 'three', 'two'])
    expect(retried.done).toBe(3)
  })

  it('re-runs a rate-limited case on retry, since it measured nothing', async () => {
    // The two fixtures ask for different picks, which is also how the throttle
    // below tells them apart: the rendered prompt is `pick <want>`.
    const h = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'b' },
    ])
    let busy = true
    const b = bench([reg(h)], { replies: [] })
    const gated = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          const want = /pick (\w+)/.exec(String(req.messages.at(-1)?.content))?.[1] ?? 'a'
          // 'two' is throttled on the first sweep only.
          if (busy && want === 'b') throw new Error('gateway completion 429: too many requests')
          return { kind: 'gateway' as const, text: obj(want), toolNames: [], usage: null, contractDropped: false }
        },
      },
    }

    const first = await runEvalSweep('candidate', { deps: gated, concurrency: 1, pressureBackoffMs: [1, 1, 1] })
    expect(first.cases.find((c) => c.case === 'two')?.skipped).toContain('rate limits')

    busy = false
    const retried = await runEvalSweep('candidate', { deps: gated, concurrency: 1, retryFailed: true, pressureBackoffMs: [1, 1, 1] })
    const two = retried.cases.find((c) => c.case === 'two')!
    expect(two.skipped).toBeNull()
    expect(two.task).toBe('pass')
  })

  it('records when each case started and what it COST the sweep, not just its latency', async () => {
    // `latencyMs` is the runner's measure of the FINAL attempt and has to stay
    // exactly what production records — it is what observed-vs-tested compares.
    // So it cannot answer either question a speed comparison asks: what did the
    // case cost (retries included), and what was running alongside it.
    const hang = new Promise<never>(() => {})
    let n = 0
    const b = bench([reg(picker('picker', [{ name: 'one', want: 'a' }]))], {
      replies: [hang, obj('a')],
      onCall: () => {
        n++
      },
    })

    const before = Date.now()
    const sweep = await runEvalSweep('candidate', { deps: b.deps, caseTimeoutMs: 40, pressureBackoffMs: [1] })
    const c = sweep.cases[0]!

    // The first request vanished and was re-asked, so the case cost the sweep
    // far more than the surviving attempt's latency says.
    expect(n).toBe(2)
    expect(c.wallMs).toBeGreaterThanOrEqual(40)
    expect(c.wallMs).toBeGreaterThan(c.latencyMs)
    // And it is placeable on a timeline.
    expect(Date.parse(c.startedAt)).toBeGreaterThanOrEqual(before)
    expect(Date.parse(c.startedAt)).toBeLessThanOrEqual(Date.now())
  })

  it('SUPPLEMENTS: runs only fixtures no archived run has answered', async () => {
    // The mode that matters once a suite is under active development. The
    // registry gained fixtures on nine harnesses this month; a model tested
    // before them had no verdict on any. Resume cannot help (the run is done, so
    // nothing is pending) and restart re-buys everything to ask the new ones.
    const two = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(two)], { replies: [obj('a')] })
    const first = await runEvalSweep('candidate', { deps: b.deps, concurrency: 1 })
    expect(first.done).toBe(2)
    const spent = b.calls.length

    // A third fixture appears in the registry.
    const three = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
      { name: 'three', want: 'a' },
    ])
    const grown = { ...b.deps, harnesses: async () => [reg(three)] }

    const after = await runEvalSweep('candidate', { deps: grown, concurrency: 1, supplement: true })

    // ONE call for the one new question.
    expect(b.calls.length - spent).toBe(1)
    expect(after.done).toBe(3)
    expect(after.cases.map((c) => c.case).sort()).toEqual(['one', 'three', 'two'])
  })

  it('PRUNES a verdict about a fixture that no longer exists', async () => {
    // A recorded case whose assertion has been deleted still scores the matrix,
    // which means the model is being judged on a question the suite stopped
    // asking. A supplemental pass is exactly the pass whose subject is the
    // difference between the ledger and the registry, so it is where this
    // belongs.
    const three = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'gone', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(three)], { replies: [obj('a')] })
    await runEvalSweep('candidate', { deps: b.deps, concurrency: 1 })

    const shrunk = {
      ...b.deps,
      harnesses: async () => [
        reg(
          picker('picker', [
            { name: 'one', want: 'a' },
            { name: 'two', want: 'a' },
          ]),
        ),
      ],
    }
    const after = await runEvalSweep('candidate', { deps: shrunk, concurrency: 1, supplement: true })

    expect(after.cases.map((c) => c.case).sort()).toEqual(['one', 'two'])
    expect(after.done).toBe(2)
  })

  it('measures speed over the cases THIS pass ran, not the ones it inherited', async () => {
    // Otherwise a supplemental pass of one fixture would report a latency
    // computed from two hundred and forty inherited cases measured last week at
    // a different width — a number about neither this pass nor this deployment.
    const two = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
    ])
    const b = bench([reg(two)], { replies: [obj('a')] })
    const first = await runEvalSweep('candidate', { deps: b.deps, concurrency: 1 })
    expect(first.measured).toHaveLength(2)

    const three = picker('picker', [
      { name: 'one', want: 'a' },
      { name: 'two', want: 'a' },
      { name: 'three', want: 'a' },
    ])
    const after = await runEvalSweep('candidate', {
      deps: { ...b.deps, harnesses: async () => [reg(three)] },
      concurrency: 1,
      supplement: true,
    })

    // The ledger is whole; the MEASUREMENT is just the new one.
    expect(after.cases).toHaveLength(3)
    expect(after.measured.map((c) => c.case)).toEqual(['three'])
  })

  it('SUPPLEMENTS a capability the model lacks and the install has', async () => {
    // THE HALF THAT WAS MISSING. `RoleFloor.suppliable` already let the run
    // proceed when a tool could stand in — that is how `research-search` avoids
    // refusing a model that cannot browse. But the sweep then handed that model
    // the ordinary transport with no tool on it, so it answered from memory and
    // the fixture failed it for having no sources. Production picks the tool
    // transport for exactly this case; now the benchmark does too, so it
    // measures what an admin assigns rather than the bare weights.
    const needsSearch = {
      ...reg(
        defineHarness<{ q: string }, string>({
          id: 'searcher',
          label: 'Searcher',
          job: 'Answers from the live web.',
          requires: ['search'],
          floor: { capabilities: ['search'], refuseBelow: true, suppliable: ['search'], note: 'needs live search' },
          model: { chain: [] },
          render: () => [{ role: 'user', content: 'what shipped this week?' }],
          output: { kind: 'text', clean: (raw) => raw.trim() || null },
          onFailure: 'null',
          evals: [{ name: 'answers with what the tool returned', input: { q: 'go' }, check: (v: string) => (v.includes('shipped') ? null : 'answered from memory') }],
        }),
      ),
    }

    const offered: string[][] = []
    const b = bench([needsSearch], { replies: [''] })
    const withTool = {
      ...b.deps,
      // The install HAS a search tool.
      supplier: async () => ({ server: 'talaria', tool: 'web_search' }),
      harnessDeps: {
        ...b.deps.harnessDeps,
        // The model cannot search natively, and the floor would refuse it — but
        // `suppliable` + a reachable tool is what lets the run happen at all.
        missingCapabilities: async () => ['search' as const],
        reach: async () => ({ search: { capability: 'search' as const, reached: true, via: 'tool' as const, supplier: { server: 'talaria', tool: 'web_search' }, detail: 'x' } }),
        transport: async (req: TransportRequest) => {
          offered.push((req.toolDefs ?? []).map((t) => t.name))
          // First turn: call the tool. Second: answer from what came back.
          return offered.length === 1
            ? { kind: 'gateway' as const, text: '', toolNames: [], toolCalls: [{ name: 'web_search', args: JSON.stringify({ query: 'shipped this week' }) }], usage: null, contractDropped: false }
            : { kind: 'gateway' as const, text: 'The ledger migration shipped on Friday.', toolNames: [], usage: null, contractDropped: false }
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: withTool, concurrency: 1 })

    // THE TOOL WAS ON THE REQUEST. Without this the model is handed nothing and
    // the fixture measures whether it can guess.
    expect(offered[0]).toEqual(['web_search'])
    // And the case reached a verdict rather than being refused by the floor.
    expect(sweep.cases[0]?.skipped).toBeNull()
    expect(sweep.cases[0]?.answered).toBe(true)
  })

  it('gives a SUPPLEMENTED case the clock its loop actually needs', async () => {
    // THE BUG. A supplemented case runs inside `toolSearchTransport` — up to
    // three search turns plus one to write the answer — and `turnsPerCase`
    // handed that whole loop the budget for ONE model turn. glm-5.2 filed three
    // research-search cases as "did not finish inside 60000ms after 1 upstream
    // call(s)", which reads as a hung request and was really a four-turn job on
    // a one-turn clock.
    const def = defineHarness<{ q: string }, string>({
      id: 'searcher',
      label: 'Searcher',
      job: 'Answers from the live web.',
      requires: ['search'],
      floor: { capabilities: ['search'], refuseBelow: true, suppliable: ['search'], note: 'Needs live search.' },
      model: { chain: [] },
      render: () => [{ role: 'user', content: 'go' }],
      output: { kind: 'text', clean: (raw) => raw.trim() || null },
      onFailure: 'null',
      evals: [{ name: 'a', input: { q: 'go' }, check: () => null }],
    })
    // Not dry-run, not supplied: one model turn, as before.
    expect(turnsPerCase(def as never, false, false)).toBe(1)
    // Supplied: the loop the transport actually runs.
    expect(turnsPerCase(def as never, false, true)).toBe(4)
  })

  it('files OUR GAP when the supplied search tool finds nothing, instead of failing the model', async () => {
    // THE INVERSION THIS FIXES. Asked what NIST 800-53 AC-2 requires, a model
    // called the search tool four times, got back NIST's homepage and the NIST
    // Chemistry WebBook, and then did exactly what the harness asks — refused to
    // supply the control text from memory. The sweep scored that a task failure.
    // The better the model behaved, the worse it scored.
    const needsSearch = {
      ...reg(
        defineHarness<{ q: string }, string>({
          id: 'searcher',
          label: 'Searcher',
          job: 'Answers from the live web.',
          requires: ['search'],
          floor: { capabilities: ['search'], refuseBelow: true, suppliable: ['search'], note: 'Needs live search.' },
          model: { chain: [] },
          render: () => [{ role: 'user', content: 'what does AC-2 require?' }],
          output: { kind: 'text', clean: (raw) => raw.trim() || null },
          onFailure: 'null',
          evals: [{ name: 'answers with dense specifics', input: { q: 'go' }, check: (v: string) => (v.includes('AC-2 requires') ? null : 'answered with no specifics') }],
        }),
      ),
    }
    const b = bench([needsSearch], { replies: [''] })
    let turn = 0
    const barren = {
      ...b.deps,
      supplier: async () => ({ server: 'talaria', tool: 'web_search' }),
      harnessDeps: {
        ...b.deps.harnessDeps,
        missingCapabilities: async () => ['search' as const],
        reach: async () => ({ search: { capability: 'search' as const, reached: true, via: 'tool' as const, supplier: { server: 'talaria', tool: 'web_search' }, detail: 'x' } }),
        transport: async () => {
          turn++
          // Calls the tool, then honestly reports that nothing came back.
          return turn === 1
            ? { kind: 'gateway' as const, text: '', toolNames: [], toolCalls: [{ name: 'web_search', args: JSON.stringify({ query: 'AC-2' }) }], usage: null, contractDropped: false }
            : { kind: 'gateway' as const, text: 'The results did not answer the question and I will not supply it from memory.', toolNames: [], usage: null, contractDropped: false }
        },
      },
    }

    // The tool answers, and finds NOTHING citable — which is what a
    // CAPTCHA-walled search backend looks like from in here.
    const sweep = await runEvalSweep('candidate', { deps: { ...barren, searchTool: async () => ({ text: 'No results for that query.', structured: [] }) }, concurrency: 1 })

    const one = sweep.cases[0]
    expect(one?.gap).toContain('returned nothing citable')
    // NOT scored against the model: a gap is unscored, and `taskError` is
    // cleared so nothing downstream reads it as a wrong answer.
    expect(one?.task).toBe('unscored')
    expect(one?.taskError).toBeNull()
  })

  it('does not supplement when the install has no tool for it', async () => {
    // The floor's refusal is a real tier-2 result — an org with neither a
    // searching model nor a search server genuinely cannot do research, and the
    // sweep must keep saying so rather than pretending a tool exists.
    const needsSearch = {
      ...reg(
        defineHarness<{ q: string }, string>({
          id: 'searcher',
          label: 'Searcher',
          job: 'Answers from the live web.',
          requires: ['search'],
          floor: { capabilities: ['search'], refuseBelow: true, suppliable: ['search'], note: 'needs live search' },
          model: { chain: [] },
          render: () => [{ role: 'user', content: 'what shipped this week?' }],
          output: { kind: 'text', clean: (raw) => raw.trim() || null },
          onFailure: 'null',
          evals: [{ name: 'answers', input: { q: 'go' }, check: () => null }],
        }),
      ),
    }
    const offered: string[][] = []
    const b = bench([needsSearch], { replies: ['from memory, probably the ledger thing'] })
    const withoutTool = {
      ...b.deps,
      supplier: async () => null,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          offered.push((req.toolDefs ?? []).map((t) => t.name))
          return { kind: 'gateway' as const, text: 'from memory', toolNames: [], usage: null, contractDropped: false }
        },
      },
    }

    await runEvalSweep('candidate', { deps: withoutTool, concurrency: 1 })
    expect(offered[0]).toEqual([])
  })

  it('records a FLOOR REFUSAL as a skip, not as a failed fixture', async () => {
    // THE RUN THAT FOUND THIS. The health view showed glm-5.2 failing five
    // `research-search` fixtures on `"glm-5.2" cannot run harness
    // "research-search"` — a refusal it never saw, recorded as five wrong
    // answers. The capability floor exists to make "we did not ask" visible, and
    // the code reading its verdict was turning that into "the model got it
    // wrong", which is the exact category error the floor is built to prevent.
    const needsSearch = {
      ...reg(
        defineHarness<{ q: string }, string>({
          id: 'searcher',
          label: 'Searcher',
          job: 'Answers from the live web.',
          requires: ['search'],
          floor: { capabilities: ['search'], refuseBelow: true, note: 'Needs live search.' },
          model: { chain: [] },
          render: () => [{ role: 'user', content: 'what shipped?' }],
          output: { kind: 'text', clean: (raw) => raw.trim() || null },
          onFailure: 'null',
          evals: [{ name: 'answers', input: { q: 'go' }, check: () => null }],
        }),
      ),
    }
    const b = bench([needsSearch], { replies: [''] })
    const refused = {
      ...b.deps,
      supplier: async () => null,
      harnessDeps: {
        ...b.deps.harnessDeps,
        // MEASURED unable, and nothing supplies it: the floor refuses.
        missingCapabilities: async () => ['search' as const],
        capabilities: async () => ({ search: { value: false, source: 'probe' as const, at: '2026-08-11T00:00:00.000Z' } }),
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: refused, concurrency: 1 })

    const one = sweep.cases[0]
    // A SKIP, which every consumer already reads as "no evidence" — not a
    // contract failure with a sentence about the model attached.
    expect(one?.skipped).toContain('cannot run harness')
    expect(one?.task).toBe('unscored')
    // And the harness's own column counts it as an absence rather than a case.
    expect(sweep.harnesses[0]).toMatchObject({ cases: 0, skipped: 1 })
  })

  it('records an unreachable model as UNMEASURED, not as a failure', async () => {
    // THE RUN THAT FOUND THIS. A sweep of qwen3.8-max recorded 247 failures,
    // every one `gateway completion 404: No allowed providers are available for
    // the selected model`, in 58ms each — the org's own no-train policy pinned
    // `provider.only` to the US pool and that model is served only by alibaba.
    // A real finding, reported as a model that fails every harness in Talaria.
    const b = bench([reg(picker('picker', [{ name: 'one', want: 'a' }]))], { replies: [obj('a')] })
    const blocked = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async () => {
          throw new Error('gateway completion 404: {"error":{"message":"No allowed providers are available for the selected model."}}')
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: blocked, concurrency: 1 })
    const c = sweep.cases[0]!

    expect(c.skipped).toContain('could not reach this model')
    expect(c.task).toBe('unscored')
    // Excluded from every rate, because nothing about the model was measured.
    expect(sweep.harnesses[0]?.cases).toBe(0)
    expect(sweep.harnesses[0]?.skipped).toBe(1)
  })

  it('STOPS after a streak of unreachable cases instead of buying the whole sweep', async () => {
    // Two hundred and forty more cases each buy the same 404 and tell an admin
    // nothing they did not know by case three.
    const many = Array.from({ length: 8 }, (_, i) => reg(picker(`h${i}`, [{ name: 'one', want: 'a' }])))
    let calls = 0
    const b = bench(many, { replies: [obj('a')] })
    const blocked = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async () => {
          calls++
          throw new Error('gateway completion 404: No allowed providers are available for the selected model.')
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: blocked, concurrency: 1 })

    expect(calls).toBeLessThanOrEqual(4)
    expect(sweep.cases.length).toBeLessThan(8)
    // And it says why ONCE, rather than leaving it to be inferred from a wall of
    // identical case errors.
    expect(sweep.error).toContain('could not reach this model')
  })

  it('does not stop on an isolated refusal among real results', async () => {
    // One 404 among passes is a blip worth recording and ignoring; the streak is
    // what makes it a fact about the whole run.
    let n = 0
    const many = Array.from({ length: 6 }, (_, i) => reg(picker(`h${i}`, [{ name: 'one', want: 'a' }])))
    const b = bench(many, { replies: [obj('a')] })
    const flaky = {
      ...b.deps,
      harnessDeps: {
        ...b.deps.harnessDeps,
        transport: async (req: TransportRequest) => {
          if (++n === 2) throw new Error('gateway completion 404: no allowed providers')
          return b.deps.harnessDeps.transport!(req)
        },
      },
    }

    const sweep = await runEvalSweep('candidate', { deps: flaky, concurrency: 1 })
    expect(sweep.cases).toHaveLength(6)
    expect(sweep.error).toBeNull()
  })

  it('names the harnesses no fixture ever tests', async () => {
    const bare = defineHarness<{ q: string }, string>({
      id: 'bare',
      label: 'Bare',
      job: 'Declares no fixtures.',
      requires: [],
      floor: FLOOR,
      model: { chain: [] },
      render: () => [{ role: 'user', content: 'go' }],
      output: { kind: 'text' },
      onFailure: 'null',
    })
    const h = picker('picker', [{ name: 'one', want: 'a' }])
    const b = bench([reg(bare), reg(h)], { replies: [obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps })
    // Not passing and not failing — invisible, which an admin reading a green
    // matrix has to be told.
    expect(sweep.unfixtured).toEqual(['bare'])
    expect(sweep.harnesses.map((s) => s.id)).toEqual(['picker'])
  })

  it('prices what it spent and carries the token counts through', async () => {
    const h = picker('picker', [{ name: 'one', want: 'a' }])
    const b = bench([reg(h)], { replies: [obj('a')] })

    const sweep = await runEvalSweep('candidate', { deps: b.deps, only: ['picker'] })
    expect(sweep.harnesses[0]?.promptTokens).toBe(40)
    expect(sweep.harnesses[0]?.completionTokens).toBe(10)
    expect(sweep.harnesses[0]?.costUsd).toBeCloseTo(50 / 1_000_000)
    expect(sweep.harnesses[0]?.estimated).toBe(false)
  })

  it('drives the REAL registry — every shipped harness, every shipped fixture', async () => {
    // The claim tier 2 rests on is that it is a driver over registry.ts and not
    // a new subsystem. This is that claim, executed: the actual 23 builtin
    // harnesses, their actual fixture inputs, through the actual runner. It
    // catches the two things a hand-written driver gets wrong — a `render` that
    // throws on its own fixture, and a harness whose declaration makes it
    // unrunnable — neither of which any per-harness unit test can see.
    const registry = builtinActivityHarnesses()
    const fixtures = registry.reduce((n, h) => n + h.evalNames.length, 0)
    expect(registry.length).toBeGreaterThanOrEqual(20)
    expect(fixtures).toBeGreaterThanOrEqual(50)

    // One canned reply for every harness in the product: most contracts will
    // reject it, which is the point — a sweep against a hopeless model must
    // still finish and score every column.
    const b = bench(registry, { replies: ['{"nope": true}'] })
    const sweep = await runEvalSweep('a-hopeless-model', { deps: b.deps, caseTimeoutMs: 5_000 })

    expect(sweep.state).toBe('done')
    expect(sweep.total).toBe(fixtures)
    expect(sweep.done).toBe(fixtures)
    // Every harness that declares a fixture gets a column; none is skipped by a
    // throw, a hang or a failure policy.
    const withFixtures = registry.filter((h) => h.evalNames.length > 0).map((h) => h.id)
    expect(sweep.harnesses.map((s) => s.id).sort()).toEqual([...withFixtures].sort())
    expect(sweep.cases.every((c) => !c.timedOut)).toBe(true)
    // NO FIXTURE MAY PASS ON A FOURTEEN-CHARACTER NON-ANSWER. This is now zero,
    // and it is the assertion that keeps `taskScore` meaning something.
    //
    // The reply above is the literal string `{"nope": true}`. Six fixtures used
    // to score it as a PASS — two in the summarizer, one in the distiller, two
    // in the briefer, one in outreach — all of them text harnesses whose `clean`
    // is `raw.trim() || null`, so the string is a legitimate value and
    // `schema_valid` was honestly true. The CONTRACT was never lying. What was
    // one-sided was the fixture's own `check`: each asserted only that the
    // answer was not too long, not markdown, not a question, not a repeat. All
    // real failure modes, all satisfied by saying almost nothing, so a hopeless
    // model was credited for six cases it did not answer.
    //
    // Each of the six now states a floor (`belowAnswerFloor` in define.ts): how
    // short is too short to be an answer, and where the input has an
    // unmistakable subject, one of a few words the answer has to have engaged
    // with. A NEW fixture that trips this is a new one-sided assertion and
    // should be written with a floor rather than admitted here.
    const passedOnGarbage = sweep.cases.filter((c) => c.task === 'pass').map((c) => caseKey(c.harness, c.case))
    expect(passedOnGarbage).toEqual([])

    // Every case that FAILED something carries the drill-down an admin needs —
    // the actual prompt and the actual reply, which is what makes a red cell
    // trustworthy instead of merely alarming.
    expect(sweep.cases.filter((c) => c.task !== 'pass').every((c) => c.prompt !== null)).toBe(true)
  })

  it('reports whether a harness can express the input-relational half of its contract', async () => {
    const naked = defineHarness<{ q: string }, Pick>({
      id: 'naked',
      label: 'Naked',
      job: 'A schema and nothing else.',
      requires: ['json'],
      floor: FLOOR,
      model: { chain: [] },
      render: () => [{ role: 'user', content: 'go' }],
      output: { kind: 'json', schema: PICK },
      onFailure: 'null',
      evals: [{ name: 'one', input: { q: 'x' }, check: () => null }],
    })
    // `verifies` is the tell for an `optimistic` count that is a bug rather
    // than a quality score: a harness with no `verify` has no way to state the
    // half of its contract a schema cannot.
    expect(metaOf(reg(naked)).verifies).toBe(false)
    expect(metaOf(reg(picker('picker', [{ name: 'one', want: 'a' }]))).verifies).toBe(true)

    // And whether a repair turn is even reachable, which follows `maxRepairs`
    // in run.ts rather than a second reading of the same rule.
    expect(metaOf(reg(naked)).repairable).toBe(true)
    const text = defineHarness<{ q: string }, string>({
      id: 'text',
      label: 'Text',
      job: 'Text out, so the runner never repairs it.',
      requires: [],
      floor: FLOOR,
      model: { chain: [] },
      render: () => [{ role: 'user', content: 'go' }],
      output: { kind: 'text' },
      onFailure: 'null',
    })
    expect(metaOf(reg(text)).repairable).toBe(false)
  })
})

describe('the clock a case races', () => {
  const jsonDef = { output: { kind: 'json' as const, schema: {} as never, repair: 1 }, tools: 'none' as const }
  const textDef = { output: { kind: 'text' as const }, tools: 'none' as const }

  it('bills a single-shot case one turn, and a repairable one two', () => {
    expect(turnsPerCase(textDef as never, false)).toBe(1)
    expect(turnsPerCase(jsonDef as never, false)).toBe(2)
  })

  it('bills a DRY RUN for the whole tool loop, which is what timed them out', () => {
    // A flat 60s was a single-call budget applied to a case that drives up to
    // MAX_TURNS model calls plus a repair. The harnesses that called the most
    // timed out the most — research-search 22%, workbench:standard 15% — and
    // every one of those was charged to the model as a contract failure.
    expect(turnsPerCase(jsonDef as never, true)).toBe(MAX_TURNS + 1)
    expect(turnsPerCase(textDef as never, true)).toBe(MAX_TURNS)
  })

  it('scales with a raised turn budget rather than being left behind', () => {
    // Read off the same constant the loop uses, so raising MAX_TURNS cannot
    // silently leave the clock at the old figure.
    expect(turnsPerCase(textDef as never, true)).toBe(MAX_TURNS)
  })
})

describe('a gap in OUR test environment', () => {
  it('is never scored as the model failing', () => {
    // THE PRINCIPLE. A fixture asserts that a coding run ran the tests, or that a
    // session filed a capability gap, or that a brief named the one blocked item
    // — and each is only a fair question if the run was actually given a test
    // runner, a gap tool, and a briefing containing that item. Where it was not,
    // the model can do everything right and still miss the assertion, and
    // scoring that as a model failure measures our fixture and calls it a
    // capability.
    const cases = [
      score({ case: 'a', task: 'pass' }),
      score({ case: 'b', task: 'unscored', gap: 'the sandbox offered no run_tests tool, so "did it verify" cannot be asked' }),
      score({ case: 'c', task: 'fail', taskError: 'left the bug in place' }),
    ]
    const s = scoreHarness(META, cases)

    // One pass, one real failure. The gap is in NEITHER half of the ratio.
    expect(s.taskScore).toBe(0.5)
    expect(s.gaps).toBe(1)
    expect(s.gapReasons).toEqual(['the sandbox offered no run_tests tool, so "did it verify" cannot be asked'])
  })

  it('reports the reason so it reaches whoever owns the harness', () => {
    // A gap that only decremented a denominator would be indistinguishable from
    // a fixture nobody wrote. The sentence is the point: it is a bug report
    // about the test environment, addressed to us.
    const s = scoreHarness(META, [score({ task: 'unscored', gap: 'no briefing was supplied for the item the assertion names' })])

    expect(s.taskScore).toBeNull()
    expect(s.gapReasons[0]).toContain('no briefing was supplied')
  })
})

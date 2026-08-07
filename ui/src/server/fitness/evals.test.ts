import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import {
  caseKey,
  IDLE_STATUS,
  metaOf,
  runEvalSweep,
  scoreHarness,
  stopEvalSweep,
  type EvalCaseScore,
  type EvalSweepStatus,
  type HarnessMeta,
} from '@/server/fitness/evals'
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
      return { kind: 'gateway', text: reply, toolNames: [], usage: { promptTokens: 40, completionTokens: 10 }, contractDropped: false }
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
      now: () => 1_700_000_000_000,
    },
  }
  return b
}

const obj = (pick: string): string => JSON.stringify({ pick })

// ── Scoring ──────────────────────────────────────────────────────────────────

const META: HarnessMeta = { id: 'h', label: 'H', source: 'builtin', outputKind: 'json', requires: ['json'], verifies: true, repairable: true }

/** A recorded case, defaulted to a clean pass so each test states only the axis
 *  it is about. */
const score = (over: Partial<EvalCaseScore>): EvalCaseScore => ({
  harness: 'h',
  case: 'c',
  contractHeld: true,
  firstPass: true,
  repairs: 0,
  answered: true,
  task: 'pass',
  taskError: null,
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
    expect(s.guardRate).toBeCloseTo(2 / 3)
    expect(s.timeouts).toBe(1)
    expect(s.scored).toBe(2)
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

    const sweep = await runEvalSweep('candidate', { deps: b.deps, caseTimeoutMs: 30 })

    expect(n).toBe(2)
    const stuckCase = sweep.cases.find((c) => c.harness === 'stuck')
    expect(stuckCase?.timedOut).toBe(true)
    expect(stuckCase?.contractHeld).toBe(false)
    expect(stuckCase?.error).toContain('did not finish inside 30ms')
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
      // Stop after the first case has been answered. The stop lands on a case
      // boundary, never mid-case.
      onCall: (n) => {
        if (n === 1) stopEvalSweep()
      },
    })

    const stopped = await runEvalSweep('candidate', { deps: b.deps })

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
    const resumed = await runEvalSweep('candidate', { deps: b.deps })
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
    await runEvalSweep('candidate-a', { deps: b.deps })
    expect(b.status.cases).toHaveLength(1)

    // A different model discards the ledger: a matrix cell assembled from two
    // models is a number with no referent.
    const other = await runEvalSweep('candidate-b', { deps: b.deps })
    expect(other.model).toBe('candidate-b')
    expect(other.done).toBe(2)
    expect(other.cases.every((c) => c.contractHeld)).toBe(true)
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

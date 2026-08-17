import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import { RULES, type Finding, type GuardConfig } from '@/server/guardrails'
import { defineHarness, type HarnessDefinition, type Message } from '@/server/harness/define'
import { gatewayStream, gatewayTransport, runHarness, runHarnessStreamed, type HarnessDeps, type HarnessRunRow, type TransportReply, type TransportRequest } from '@/server/harness/run'
import { personaKeysFrom, type PersonaRow } from '@/server/harness/persona'
import type { Capability, CapabilitySource } from '@/server/harness/capability'

// The runner is exercised end to end against RECORDED REPLIES. That is not a
// convenience: the whole point of the harness layer is that a 14B model returns
// something a frontier model would not, and the only way to hold that behavior
// still is to write the bad reply down and assert what the runner does with it.
// Every edge — model resolution, capability facts, the transport, the guard
// config, findings, the harness_runs row, the clock — is a field on
// `HarnessDeps`, so nothing here touches a database, a gateway or a fleet.
//
// Three things are deliberately REAL rather than faked, because faking them
// would turn the assertion into a restatement of the fake:
//   - the guard pass (`runGuardrails` runs the actual rule registry)
//   - the parser (`parseJson`, via the harness's own zod schema)
//   - the persona resolver (`personaKeysFrom`, over recorded agent config rows —
//     tier resolution is the whole question, so a fake that returned keys would
//     be testing nothing)

const VERDICT = z.object({ verdict: z.enum(['pass', 'revise']), summary: z.string() })
type Verdict = z.infer<typeof VERDICT>

/** A judge-shaped harness: a real output contract, a real floor, and a
 *  capability it refuses to work without. */
const judge = defineHarness<{ ticket: string }, Verdict>({
  id: 'judge',
  label: 'Judge',
  job: 'Reviews an agent-reported outcome against the ticket.',
  requires: ['json', 'json-strict'],
  floor: {
    capabilities: ['json'],
    refuseBelow: true,
    note: 'A judge that cannot return a structured verdict escalates everything, which is a notification storm rather than a review.',
  },
  model: { pin: 'judge' },
  render: (input, ctx) => [
    { role: 'system', content: ctx.widened ? 'You may also cite the diff.' : 'Judge the reported outcome.' },
    { role: 'user', content: input.ticket },
  ],
  output: { kind: 'json', schema: VERDICT },
  onFailure: 'null',
  temperature: 0,
})

/** A titler-shaped harness: text out, almost nothing declared, and it must run
 *  on whatever the self-host has. */
const titler = defineHarness<{ transcript: string }, string>({
  id: 'titler',
  label: 'Titler',
  job: 'Names a conversation once its first exchange lands.',
  requires: [],
  floor: { capabilities: [], refuseBelow: false, note: 'Runs on anything — a mediocre title beats no title.' },
  model: { pin: 'titler' },
  render: (input) => [{ role: 'user', content: input.transcript }],
  output: {
    kind: 'text',
    clean: (raw) => {
      const line = (raw.split('\n').find((l) => l.trim()) ?? '').replace(/^["'\s]+|["'\s]+$/g, '')
      return line || null
    },
  },
  onFailure: 'null',
})

// ── The fake world ───────────────────────────────────────────────────────────

interface World {
  /** Replies the transport hands back, in order. The last one repeats. */
  replies?: Array<string | TransportReply>
  model?: { model: string; step: 'pin' | 'role' | 'utility' | 'env' | 'preferred' | 'first-routable' } | null
  endpoints?: string[]
  /** Capability facts per endpoint name. Absent = UNKNOWN, which is the state
   *  a fresh self-host is in and the state most of these cases care about.
   *  A bare boolean is a `probe` fact — a deliberate measurement — because that
   *  is what most cases here mean; `{ value, source }` spells out the rest, and
   *  `learned` (what the gateway writes off a 400) behaves differently at the
   *  floor. */
  facts?: Record<string, Partial<Record<Capability, boolean | { value: boolean; source: CapabilitySource }>>>
  guardMode?: GuardConfig['mode']
  /** Agent versions the persona resolver reads. Supplying these implies the
   *  model is NOT a gateway catalog model — `routingFor` answers with no
   *  endpoints for a persona — so `endpoints` defaults to none here. */
  personas?: PersonaRow[]
  /** The config lookup throws (the database is down mid-run). */
  personasThrow?: boolean
}

interface Recorder {
  requests: TransportRequest[]
  runs: HarnessRunRow[]
  recorded: Finding[]
  deps: Partial<HarnessDeps>
}

const DEFAULT_CONFIG: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

/** The gate-safe rules over plain text — what `guardText` does, minus its
 *  settings read. The runner calls it before it repairs, and this is the pass
 *  that decides whether a bad reply is safe to hand back to a model. */
const gateSafe = (text: string, input?: string): Finding[] => {
  const out: Finding[] = []
  for (const rule of RULES) {
    if (!rule.gateSafe) continue
    const hit = rule.run({
      answer: text,
      toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false },
      userMessage: '',
      ...(input ? { inputText: input } : {}),
      policedHosts: [],
    })
    // The real `guardText` drops a `finding+redaction` hit here rather than
    // returning it; the stub has to as well, or the repair gate would refuse a
    // reply over a finding that never exists in production.
    if (hit && !(hit.grounded && rule.groundable === 'finding+redaction')) {
      out.push({
        check: rule.id,
        severity: rule.severity,
        confidence: hit.confidence,
        message: hit.message,
        snippet: hit.snippet,
        ...(hit.grounded && rule.groundable ? { grounded: true } : {}),
      })
    }
  }
  return out
}

function world(w: World = {}): Recorder {
  const requests: TransportRequest[] = []
  const runs: HarnessRunRow[] = []
  const recorded: Finding[] = []
  const replies = w.replies ?? ['{"verdict":"pass","summary":"looks right"}']
  // A persona is not on the gateway catalog, so `routingFor` finds no endpoints
  // for it — which is precisely the condition that used to leave `keys` empty.
  const endpoints = w.endpoints ?? (w.personas || w.personasThrow ? [] : ['spark'])
  let clock = 0

  const factsFor = (key: string): Partial<Record<Capability, { value: boolean; source?: CapabilitySource }>> => {
    // Keys are 'endpoint:model' (capability.ts). A full key in `facts` wins;
    // otherwise the bare endpoint name applies to every model it serves, which
    // is the shorthand most cases here want.
    const endpoint = key.slice(0, key.indexOf(':'))
    const known = w.facts?.[key] ?? w.facts?.[endpoint] ?? {}
    const out: Partial<Record<Capability, { value: boolean; source?: CapabilitySource }>> = {}
    for (const [cap, fact] of Object.entries(known)) {
      out[cap as Capability] = typeof fact === 'boolean' ? { value: fact, source: 'probe' } : fact
    }
    return out
  }

  return {
    requests,
    runs,
    recorded,
    deps: {
      resolveModel: async () => (w.model === undefined ? { model: 'pl-main', step: 'pin' } : w.model),
      routing: async (model) => ({ endpoints, upstreamModel: model }),
      // The REAL resolver over recorded agent-version rows. Tier resolution is
      // the whole question here, so a fake that just handed back keys would be
      // asserting nothing.
      personaKeys: async (model) => {
        if (w.personasThrow) throw new Error('connection terminated unexpectedly')
        return personaKeysFrom(model, w.personas ?? [])
      },
      // The cardinal rule of capability.ts, reproduced exactly: only a fact
      // that positively says "no" counts as missing. Unknown is not missing.
      missingCapabilities: async (key, required) => {
        const facts = factsFor(key)
        return required.filter((cap) => facts[cap]?.value === false)
      },
      capabilities: async (key) => factsFor(key),
      transport: async (req) => {
        requests.push(req)
        const reply = replies[Math.min(requests.length - 1, replies.length - 1)] ?? ''
        if (typeof reply !== 'string') return reply
        return { kind: 'gateway', text: reply, toolNames: [], usage: null, contractDropped: false }
      },
      guardConfig: async () => ({ ...DEFAULT_CONFIG, mode: w.guardMode ?? 'observe' }),
      guardText: async (text, input) => gateSafe(text, input),
      recordFindings: async (findings) => {
        recorded.push(...findings)
      },
      recordRun: async (row) => {
        runs.push(row)
      },
      now: () => (clock += 7),
    },
  }
}

const run = <I, O>(def: HarnessDefinition<I, O>, input: I, r: Recorder) => runHarness(def, input, { caller: 'test:harness', deps: r.deps })

// ── Happy path ───────────────────────────────────────────────────────────────

describe('the happy path', () => {
  it('parses the value, records the winning chain step, and writes one run row', async () => {
    const r = world()
    const res = await run(judge, { ticket: 'ship the thing' }, r)

    expect(res.value).toEqual({ verdict: 'pass', summary: 'looks right' })
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(0)
    expect(res.model).toBe('pl-main')
    // Which fallback actually carried the harness is part of the answer, not a
    // detail — a subsystem limping along on 'first-routable' for a month is
    // invisible without it.
    expect(res.step).toBe('pin')
    expect(res.error).toBeUndefined()
    expect(r.runs).toEqual([
      { harness: 'judge', model: 'pl-main', step: 'pin', widened: false, repairs: 0, schemaValid: true, latencyMs: 7, findings: 0, caller: 'test:harness', error: null },
    ])
  })

  it('asks for JSON at the protocol level AND anchors it in the prompt', async () => {
    // One strategy on every transport, rather than inbox-focus's two (audit
    // 1.3). The anchor is what survives a gateway that drops response_format.
    const r = world()
    await run(judge, { ticket: 'ship the thing' }, r)
    const req = r.requests[0]!
    expect(req.jsonMode).toBe(true)
    expect(req.temperature).toBe(0)
    expect(req.messages.at(-1)?.content).toContain('exactly one JSON value')
  })

  it('does not ask for protocol JSON when the model is known to refuse it, and still anchors', async () => {
    const r = world({ facts: { spark: { json: false } } })
    // The floor would refuse the judge here, so use a harness that only
    // DEGRADES below its floor — the degraded path still has to work.
    const soft = { ...judge, floor: { ...judge.floor, refuseBelow: false } }
    const res = await run(soft, { ticket: 'x' }, r)
    expect(r.requests[0]?.jsonMode).toBe(false)
    expect(r.requests[0]?.messages.at(-1)?.content).toContain('exactly one JSON value')
    expect(res.value).toEqual({ verdict: 'pass', summary: 'looks right' })
  })

  it('cleans a text harness rather than schema-parsing it', async () => {
    const r = world({ replies: ['"Migrating the ledger to Postgres"\n\nHope that helps!'] })
    const res = await run(titler, { transcript: 'a chat' }, r)
    expect(res.value).toBe('Migrating the ledger to Postgres')
    expect(r.requests[0]?.jsonMode).toBe(false)
  })
})

// ── Repair (audit 1.4) ───────────────────────────────────────────────────────

describe('repair', () => {
  it('re-asks once on a malformed reply and reports the repair', async () => {
    const r = world({
      // The shape a 14B model actually emits: a preamble, then the object, then
      // more prose — and a field the schema does not accept.
      replies: ['Sure! Here is my verdict:\n\n{"verdict": "maybe", "summary": "unclear"}\n\nLet me know if you want more.', '{"verdict":"revise","summary":"the tests are missing"}'],
    })
    const res = await run(judge, { ticket: 'ship the thing' }, r)

    expect(res.value).toEqual({ verdict: 'revise', summary: 'the tests are missing' })
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(1)
    expect(r.runs[0]?.repairs).toBe(1)
  })

  it('hands the model back its own reply plus the concrete parser error', async () => {
    const r = world({ replies: ['{"verdict": "maybe", "summary": "unclear"}', '{"verdict":"pass","summary":"fine"}'] })
    await run(judge, { ticket: 'ship the thing' }, r)

    const repair = r.requests[1]!.messages
    expect(repair.slice(0, 2)).toEqual(r.requests[0]!.messages)
    expect(repair.at(-2)).toEqual({ role: 'assistant', content: '{"verdict": "maybe", "summary": "unclear"}' })
    // The repair prompt names the FIELD, not a stack trace — that is the
    // difference between a small model fixing it and rewriting it.
    expect(repair.at(-1)?.content).toContain("field 'verdict'")
  })

  it('gives up after the declared number of repairs', async () => {
    const r = world({ replies: ['not json', 'still not json', '{"verdict":"pass","summary":"too late"}'] })
    const res = await run(judge, { ticket: 'ship the thing' }, r)

    expect(r.requests).toHaveLength(2) // one call, one repair — never the third
    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.repairs).toBe(1)
    expect(res.error).toContain('no JSON object or array was found')
  })

  it('honors a harness that asks for more than one repair round', async () => {
    const r = world({ replies: ['nope', 'still nope', '{"verdict":"pass","summary":"third time"}'] })
    const patient: HarnessDefinition<{ ticket: string }, Verdict> = { ...judge, output: { kind: 'json', schema: VERDICT, repair: 2 } }
    const res = await run(patient, { ticket: 'x' }, r)
    expect(res.repairs).toBe(2)
    expect(res.value).toEqual({ verdict: 'pass', summary: 'third time' })
  })

  it('REFUSES to repair a reply the guard flagged', async () => {
    // The repair turn is the one place this runner puts model output back into a
    // model's context. guardrails.ts's cardinal invariant — flagged content
    // never re-enters a model's context — has to hold here or it holds nowhere.
    const r = world({ replies: ['my key is AKIAIOSFODNN7EXAMPLE and here is the verdict', '{"verdict":"pass","summary":"fine"}'] })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(r.requests).toHaveLength(1)
    expect(res.value).toBeNull()
    expect(res.error).toContain('not repaired')
    expect(res.findings.some((f) => f.check === 'secret_leak')).toBe(true)
  })

  it('counts a refused repair as ONE leak, not two', async () => {
    // The repair gate and the final guard pass scan the same reply and both run
    // the gate-safe rules. Recording the leak twice would make `guard_findings`
    // — the live per-model confabulation rate the fitness page shows next to
    // benched scores — double precisely when the repair path protects us, so
    // the safety feature would read as a safety regression.
    const r = world({ replies: ['my key is AKIAIOSFODNN7EXAMPLE and here is the verdict'] })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(res.findings.filter((f) => f.check === 'secret_leak')).toHaveLength(1)
    expect(r.recorded.filter((f) => f.check === 'secret_leak')).toHaveLength(1)
    expect(r.runs[0]?.findings).toBe(1)
  })

  it('never puts a guard finding into the repair prompt', async () => {
    // The invariant, stated as an assertion: the repair turn carries the PARSER
    // error. A finding's `snippet` is a verbatim excerpt of the flagged content,
    // so interpolating one would feed the credential back to the model while
    // ostensibly enforcing the rule against doing exactly that.
    const r = world({ replies: ['{"verdict":"maybe"}', '{"verdict":"pass","summary":"ok"}'] })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(res.repairs).toBe(1)
    const repairTurn = r.requests[1]?.messages.at(-1)?.content ?? ''
    expect(repairTurn).toContain("field 'verdict'")
    expect(repairTurn).not.toMatch(/guard|flagged|redacted|leak/i)
  })
})

// ── Failure policy ───────────────────────────────────────────────────────────

describe('onFailure', () => {
  const bad = { replies: ['nope', 'nope'] }

  it("'null' returns nothing and says why", async () => {
    const res = await run(judge, { ticket: 'x' }, world(bad))
    expect(res.value).toBeNull()
    expect(res.error).toBeTruthy()
  })

  it('a declared fallback is returned but never counts as a valid contract', async () => {
    // schemaValid staying false is the point: counting a fallback as a pass
    // would quietly inflate every contract rate in the fitness matrix.
    const r = world(bad)
    const withFallback: HarnessDefinition<{ ticket: string }, Verdict> = {
      ...judge,
      onFailure: { fallback: { verdict: 'revise', summary: 'the judge could not read the outcome' } },
    }
    const res = await run(withFallback, { ticket: 'x' }, r)
    expect(res.value).toEqual({ verdict: 'revise', summary: 'the judge could not read the outcome' })
    expect(res.schemaValid).toBe(false)
    expect(r.runs[0]?.schemaValid).toBe(false)
  })

  it("'escalate' marks the result for the caller to raise", async () => {
    const res = await run({ ...judge, onFailure: { escalate: true } } as HarnessDefinition<{ ticket: string }, Verdict>, { ticket: 'x' }, world(bad))
    expect(res.value).toBeNull()
    // A FLAG, not a phrase. Only the caller knows who to tell, and a caller that
    // has to string-match the error to find out stops escalating the day
    // somebody rewords it.
    expect(res.escalate).toBe(true)
    expect(res.error).toContain('escalate to a human')
  })

  it('never sets escalate for the other three policies', async () => {
    expect((await run(judge, { ticket: 'x' }, world(bad))).escalate).toBe(false)
    const fb: HarnessDefinition<{ ticket: string }, Verdict> = { ...judge, onFailure: { fallback: { verdict: 'revise', summary: 'n/a' } } }
    expect((await run(fb, { ticket: 'x' }, world(bad))).escalate).toBe(false)
    // And a run that SUCCEEDED is never an escalation, whatever it declared.
    const esc = { ...judge, onFailure: { escalate: true } } as HarnessDefinition<{ ticket: string }, Verdict>
    expect((await run(esc, { ticket: 'x' }, world())).escalate).toBe(false)
  })

  it("'throw' still writes the run row before it throws", async () => {
    const r = world(bad)
    await expect(run({ ...judge, onFailure: 'throw' } as HarnessDefinition<{ ticket: string }, Verdict>, { ticket: 'x' }, r)).rejects.toThrow(/harness "judge" failed/)
    expect(r.runs).toHaveLength(1)
    expect(r.runs[0]?.schemaValid).toBe(false)
  })
})

// ── 'throw' means EVERY failure to produce a value ────────────────────────────
//
// It used to mean a contract failure and nothing else. `runHarness` RETURNS for
// everything that happens before or during the call, so the policy had to be
// restated by hand at each call site: five callers restated it, and the two that
// did not both shipped a bug — research synthesis saved an empty report, marked
// the run `done`, indexed it and notified the requester after a 502, and the
// channel planner answered "nothing to plan yet" on a channel full of work
// because its agent container was restarting.

describe("onFailure 'throw' on the return paths", () => {
  const throwing = { ...judge, onFailure: 'throw' } as HarnessDefinition<{ ticket: string }, Verdict>

  it('throws on a transport error', async () => {
    const r = world()
    await expect(
      runHarness(throwing, { ticket: 'x' }, {
        caller: 'test:harness',
        deps: {
          ...r.deps,
          transport: async () => {
            throw new Error('gateway completion 503')
          },
        },
      }),
    ).rejects.toThrow(/503/)
    // The row lands first, every time. A throwing harness is precisely the one
    // an operator has to be able to find in the fitness data.
    expect(r.runs).toHaveLength(1)
    expect(r.runs[0]?.error).toContain('503')
  })

  it('throws when nothing in the chain routes', async () => {
    const r = world({ model: null })
    await expect(run(throwing, { ticket: 'x' }, r)).rejects.toThrow(/no model available/)
    expect(r.runs).toHaveLength(1)
  })

  it('throws on a capability refusal, naming the capability', async () => {
    const r = world({ facts: { spark: { json: { value: false, source: 'probe' } } } })
    await expect(run(throwing, { ticket: 'x' }, r)).rejects.toThrow(/known not to support json/)
    expect(r.requests).toHaveLength(0)
  })

  it('throws when the render produces nothing', async () => {
    const r = world()
    const rendersNothing: HarnessDefinition<{ ticket: string }, Verdict> = { ...throwing, render: (): Message[] => [] }
    await expect(run(rendersNothing, { ticket: 'x' }, r)).rejects.toThrow(/rendered no messages/)
  })

  it('names no model in the sentence when there was none to name', async () => {
    const r = world({ model: null })
    await expect(run(throwing, { ticket: 'x' }, r)).rejects.toThrow(/^harness "judge" failed: /)
  })

  it('leaves the other three policies contract-scoped, deliberately', async () => {
    // Widening them would break both callers that use them: a fallback would
    // hand outreach its "nothing to surface" token during a gateway outage, so a
    // dead provider would read as a normal quiet pass on every sweep; and an
    // escalation would have the judge notify every board editor about every
    // ticket for as long as the gateway is down. `answered` is how a caller asks
    // for either on purpose.
    const r = world({ model: null })
    const fb: HarnessDefinition<{ ticket: string }, Verdict> = { ...judge, onFailure: { fallback: { verdict: 'revise', summary: 'n/a' } } }
    const fallen = await run(fb, { ticket: 'x' }, r)
    expect(fallen.value).toBeNull()
    expect(fallen.answered).toBe(false)

    const esc = { ...judge, onFailure: { escalate: true } } as HarnessDefinition<{ ticket: string }, Verdict>
    expect((await run(esc, { ticket: 'x' }, world({ model: null }))).escalate).toBe(false)
  })
})

// ── `answered`: did the model actually answer ─────────────────────────────────
//
// `raw !== null` had become the de-facto test in three adapters, and `raw` is a
// bounded drill-down field that answers a different question — it survives a
// stream that died three tokens in, so a transport failure read to all three as
// a model that answered badly.

describe('the answered field', () => {
  it('is false when nothing was reached', async () => {
    expect((await run(judge, { ticket: 'x' }, world({ model: null }))).answered).toBe(false)
    expect((await run(judge, { ticket: 'x' }, world({ facts: { spark: { json: false } } }))).answered).toBe(false)
    const r = world()
    const rendersNothing: HarnessDefinition<{ ticket: string }, Verdict> = { ...judge, render: (): Message[] => [] }
    expect((await run(rendersNothing, { ticket: 'x' }, r)).answered).toBe(false)
  })

  it('is FALSE when a stream dies mid-flight, even though a partial reply arrived', async () => {
    // THE CASE `raw !== null` GOT WRONG, and all three adapters used `raw` for
    // this question. The partial IS the diagnosis and stays on `raw`, but the
    // turn produced no answer — channel-plan's route has to say 502 here rather
    // than "nothing to plan yet".
    const r = world()
    const res = await runHarnessStreamed({ ...titler, guard: {} }, { transcript: 'x' }, { caller: 'test:harness', deps: r.deps }, {
      stream: async (_req, emit) => {
        emit('Migrating the ')
        throw new Error('socket hang up')
      },
    })

    expect(res.answered).toBe(false)
    expect(res.raw).toBe('Migrating the ')
    expect(res.error).toContain('socket hang up')
  })

  it('is TRUE for a reply the contract rejected — the model still spoke', async () => {
    const res = await run(judge, { ticket: 'x' }, world({ replies: ['Sure! I will get right on that.'] }))
    expect(res.value).toBeNull()
    expect(res.answered).toBe(true)
    expect(res.raw).toBe('Sure! I will get right on that.')
  })

  it('is false when the model answered with nothing at all', async () => {
    const res = await run(titler, { transcript: 'x' }, world({ replies: ['   '] }))
    expect(res.answered).toBe(false)
    expect(res.error).toContain('did not survive')
  })

  it('is true on a run that worked', async () => {
    expect((await run(judge, { ticket: 'x' }, world())).answered).toBe(true)
  })
})

// ── verify: the half of a contract a schema cannot state ──────────────────────
//
// A schema is a module constant. It is built at import time and cannot see the
// run's INPUT, so every harness whose correctness is a RELATION between what was
// asked and what came back had nowhere to say so — and the runner recorded
// `schemaValid: true` for a value the caller then threw away. That column is the
// OBSERVED half of the model-fitness matrix, and the offline eval fixtures
// already disagreed with it: blurb-writer's `checkBatch` rejects invented ids
// while its schema passed them.

const BLURBS = z.record(z.string(), z.string())
type Blurbs = z.infer<typeof BLURBS>

/** THE BUG, as a harness. `z.record(z.string(), z.string())` cannot constrain
 *  the KEYS, so a model that tidied `qwen3-14b` into `Qwen3 14B` passed the
 *  schema, wrote zero usable blurbs and reported a 100% contract rate — then
 *  came back around on the identical batch every ten minutes forever. */
const blurber = defineHarness<{ ids: string[] }, Blurbs>({
  id: 'blurb-writer',
  label: 'Blurb writer',
  job: 'Writes the one-line description under each model in the picker.',
  requires: ['json'],
  floor: { capabilities: [], refuseBelow: false, note: 'A blurb is a nicety; no blurb is better than a wrong one.' },
  model: { pin: 'blurb-writer' },
  render: (input) => [{ role: 'user', content: `Describe: ${input.ids.join(', ')}` }],
  output: {
    kind: 'json',
    schema: BLURBS,
    // Written as an INSTRUCTION TO THE MODEL, because that is where it goes.
    verify: (value, input) => {
      const asked = new Set(input.ids)
      const invented = Object.keys(value).filter((id) => !asked.has(id))
      if (!invented.length) return null
      return `the keys must be the model ids exactly as they were given - ${invented.map((i) => `"${i}"`).join(', ')} is not one of them. The ids are: ${input.ids.join(', ')}.`
    },
  },
  onFailure: 'null',
})

const IDS = { ids: ['qwen3-14b', 'llama-3.3-70b'] }
const TIDIED = '{"Qwen3 14B":"A capable mid-size model.","Llama 3.3 70B":"Meta\'s flagship open model."}'
const CORRECT = '{"qwen3-14b":"A capable mid-size model.","llama-3.3-70b":"Meta\'s flagship open model."}'

describe('output.verify', () => {
  it('passes a value whose relation to the input holds', async () => {
    const r = world({ replies: [CORRECT] })
    const res = await run(blurber, IDS, r)
    expect(res.value).toEqual({ 'qwen3-14b': 'A capable mid-size model.', 'llama-3.3-70b': "Meta's flagship open model." })
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(0)
  })

  it('fails the contract on a reply the SCHEMA accepted, then repairs it', async () => {
    const r = world({ replies: [TIDIED, CORRECT] })
    const res = await run(blurber, IDS, r)

    expect(res.value).toEqual({ 'qwen3-14b': 'A capable mid-size model.', 'llama-3.3-70b': "Meta's flagship open model." })
    expect(res.repairs).toBe(1)
    expect(res.schemaValid).toBe(true)
    // Same loop, same counter, same repair prompt — a verify failure IS a
    // contract failure, and the sentence the harness wrote is the instruction
    // the model gets back.
    expect(r.requests[1]?.messages.at(-1)?.content).toContain('the keys must be the model ids exactly as they were given')
    expect(r.requests[1]?.messages.at(-1)?.content).toContain('"Qwen3 14B"')
  })

  it('is a contract FAILURE when the repair does not take, on the result and on the row', async () => {
    const r = world({ replies: [TIDIED, TIDIED] })
    const res = await run(blurber, IDS, r)

    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    // The honest row is the entire point. Before this the run recorded
    // `schema_valid: true` for a batch that produced no blurbs, so the harness
    // re-burned it every ten minutes and the fitness matrix called the model a
    // 100% performer for doing it.
    expect(r.runs[0]?.schemaValid).toBe(false)
    expect(r.runs[0]?.error).toContain('the keys must be the model ids')
    // It answered — twice. That is a different fact from holding the contract.
    expect(res.answered).toBe(true)
    expect(res.repairs).toBe(1)
  })

  it('receives the ORIGINAL input on every attempt, never the repaired message list', async () => {
    // By the second turn `sent` carries the model's own rejected answer. A
    // verify graded against that would drift further from the caller's actual
    // request on every repair, which is the opposite of what a repair is for.
    const seen: Array<{ ids: string[] }> = []
    const recording: HarnessDefinition<{ ids: string[] }, Blurbs> = {
      ...blurber,
      output: {
        kind: 'json',
        schema: BLURBS,
        verify: (_value, input) => {
          seen.push(input)
          return 'never satisfied'
        },
      },
    }
    const r = world({ replies: [CORRECT, CORRECT] })
    await run(recording, IDS, r)

    expect(seen).toHaveLength(2)
    expect(seen[0]).toBe(IDS)
    expect(seen[1]).toBe(IDS)
  })

  it('a verify that throws is a contract failure, not an escaped exception', async () => {
    // Harness-author code meeting model output, exactly like `render`, `clean`
    // and `ground` — and held to the same rule, because a runner whose whole
    // promise is that a bad model produces a RESULT cannot have one function
    // that throws out of it.
    const r = world({ replies: [CORRECT] })
    const boom: HarnessDefinition<{ ids: string[] }, Blurbs> = {
      ...blurber,
      output: {
        kind: 'json',
        schema: BLURBS,
        verify: (): string => {
          throw new TypeError("Cannot read properties of undefined (reading 'length')")
        },
      },
    }
    const res = await run(boom, IDS, r)
    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.error).toContain('verify step threw')
    expect(r.runs).toHaveLength(1)
  })

  it('never runs on a value that did not parse', async () => {
    // It is defined over O, so there has to be an O. A model that answered in
    // prose is a parse failure and the parser's sentence is the repair.
    let calls = 0
    const counting: HarnessDefinition<{ ids: string[] }, Blurbs> = {
      ...blurber,
      output: {
        kind: 'json',
        schema: BLURBS,
        verify: () => {
          calls++
          return null
        },
      },
    }
    const res = await run(counting, IDS, world({ replies: ['Sure! Which models did you mean?'] }))
    expect(calls).toBe(0)
    expect(res.error).toContain('no JSON object or array was found')
  })

  it('re-checks the REDACTED value, which is the truncation class', async () => {
    // A value can survive being cut and still parse: the schema says the field
    // is a string, and only the harness can say the string still has to be the
    // thing that was asked for. The contract is re-applied whole after
    // redaction, `verify` included.
    const leaky = '{"qwen3-14b":"Ships with key AKIAIOSFODNN7EXAMPLE.","llama-3.3-70b":"Fine."}'
    const strict: HarnessDefinition<{ ids: string[] }, Blurbs> = {
      ...blurber,
      guard: { redact: true },
      output: {
        kind: 'json',
        schema: BLURBS,
        // Rejects a blurb that lost its substance to the redactor — the shape
        // survives, the value does not.
        verify: (value) => (Object.values(value).some((b) => b.includes('[redacted')) ? 'every blurb must describe the model, not the redaction that replaced its text.' : null),
      },
    }
    const r = world({ replies: [leaky] })
    const res = await run(strict, IDS, r)

    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.error).toContain('no longer satisfies the contract')
  })

  it('works on a text harness too', async () => {
    // "Did it answer the question I asked" is the same question as "are these
    // the ids I sent", and `clean` cannot ask it either — it is written before
    // the input exists.
    const echoing: HarnessDefinition<{ transcript: string }, string> = {
      ...titler,
      output: {
        kind: 'text',
        clean: (raw) => raw.trim() || null,
        verify: (value, input) => (input.transcript.startsWith(value) ? 'the title must name what the conversation is about, not repeat its opening words.' : null),
      },
    }
    const res = await run(echoing, { transcript: 'Migrating the ledger to Postgres, and what that costs' }, world({ replies: ['Migrating the ledger'] }))
    expect(res.value).toBeNull()
    expect(res.error).toContain('must name what the conversation is about')
    // A text harness does not repair: the one repair wording lives in json.ts
    // and ends "send the corrected JSON value only".
    expect(res.repairs).toBe(0)
  })
})

// ── Capability gating ────────────────────────────────────────────────────────

describe('the capability floor', () => {
  it('refuses below the floor, names the capability, and never calls the model', async () => {
    const r = world({ facts: { spark: { json: false } } })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(r.requests).toHaveLength(0)
    expect(res.value).toBeNull()
    expect(res.error).toContain('json')
    expect(res.error).toContain(judge.floor.note)
    // The refusal is still a run: an operator has to be able to see that this
    // model is being asked to do a job it cannot do.
    expect(r.runs).toEqual([
      {
        harness: 'judge',
        model: 'pl-main',
        step: 'pin',
        widened: false,
        repairs: 0,
        schemaValid: false,
        latencyMs: 7,
        findings: 0,
        caller: 'test:harness',
        // The row carries the SENTENCE, not just the false. A red cell an
        // operator cannot interrogate is a number nobody acts on.
        error: expect.stringContaining('cannot run harness "judge"') as unknown as string,
      },
    ])
  })

  it('sends the harness OWN SCHEMA, not the loose json_object that Anthropic rejects', async () => {
    // THE BUG THIS LOCKS. Every structured call carried
    // `response_format: { type: 'json_object' }`, hardcoded in three places.
    // Anthropic's OpenAI-compatible layer answers that with
    //   400  response_format.type: Input should be 'json_schema'
    // so every JSON harness 400'd on every Claude model — and the fitness suite
    // scored it as the MODEL failing to hold a contract. Nine of twenty-six
    // harnesses read 0% on claude-haiku while every text harness read 100%.
    //
    // The schema was in the harness the whole time; it was only ever used to
    // reject the reply afterwards.
    const r = world({})
    await run(judge, { ticket: 'x' }, r)

    const wire = r.requests[0]?.jsonSchema
    expect(r.requests[0]?.jsonMode).toBe(true)
    expect(wire?.schema).toMatchObject({ type: 'object', properties: { verdict: {}, summary: {} } })
    expect(wire?.name).toMatch(/^[a-zA-Z0-9_-]+$/)
  })

  it('does NOT refuse on a fact the gateway merely LEARNED from a 400', async () => {
    // The judge is the only harness with `refuseBelow`, and the gateway writes
    // `json: false` the first time an upstream rejects `response_format`. Read
    // as a floor, one 400 turned the QA gate off for every board for the 30-day
    // learned TTL — no `judge_reviews` row, no escalation, no notification, no
    // admin surface saying so. That fact is evidence about ONE PARAMETER: it
    // still shapes the request (no protocol JSON, prompt anchor only), and the
    // model still gets to answer.
    const r = world({ facts: { spark: { json: { value: false, source: 'learned' } } } })
    const res = await run(judge, { ticket: 'x' }, r)
    expect(r.requests).toHaveLength(1)
    expect(r.requests[0]?.jsonMode).toBe(false)
    expect(r.requests[0]?.messages.at(-1)?.content).toContain('exactly one JSON value')
    expect(res.value).toEqual({ verdict: 'pass', summary: 'looks right' })
  })

  it('DOES refuse on a fact somebody deliberately measured', async () => {
    for (const source of ['probe', 'declared'] as const) {
      const r = world({ facts: { spark: { json: { value: false, source } } } })
      const res = await run(judge, { ticket: 'x' }, r)
      expect(r.requests, source).toHaveLength(0)
      expect(res.error, source).toContain('cannot run harness "judge"')
    }
  })

  it('runs anyway when the harness degrades rather than refuses', async () => {
    const r = world({ facts: { spark: { json: false } } })
    const res = await run({ ...judge, floor: { ...judge.floor, refuseBelow: false } }, { ticket: 'x' }, r)
    expect(r.requests).toHaveLength(1)
    expect(res.value).not.toBeNull()
  })

  it('UNKNOWN IS NOT MISSING — an unprobed model still runs', async () => {
    // A fresh self-host has probed nothing. Talaria cannot refuse to work until
    // an admin gets around to running a benchmark.
    const r = world({ facts: {} })
    const res = await run(judge, { ticket: 'x' }, r)
    expect(r.requests).toHaveLength(1)
    expect(res.value).not.toBeNull()
  })

  it('needs the whole pool to agree before it refuses', async () => {
    // A bare model name may be served by several endpoints and we cannot know
    // which one takes this call without perturbing the round-robin cursor. One
    // member saying "no" is not knowledge about the call we are about to make.
    const r = world({ endpoints: ['spark', 'spark-2'], facts: { spark: { json: false } } })
    const res = await run(judge, { ticket: 'x' }, r)
    expect(r.requests).toHaveLength(1)
    expect(res.value).not.toBeNull()
  })
})

describe('widening', () => {
  const wide: HarnessDefinition<{ ticket: string }, Verdict> = {
    ...judge,
    widen: { requires: ['tool-select'], note: 'Models that pick the right tool get the full action list.' },
  }

  it('widens only when the capability is known TRUE', async () => {
    const r = world({ facts: { spark: { 'tool-select': true } } })
    const res = await run(wide, { ticket: 'x' }, r)
    expect(res.widened).toBe(true)
    expect(r.requests[0]?.messages[0]?.content).toContain('cite the diff')
    expect(r.runs[0]?.widened).toBe(true)
  })

  it('does NOT widen on unknown — the opposite direction to the floor, deliberately', async () => {
    // The floor asks "is this model known to be UNABLE"; widening asks "is it
    // known to be ABLE". Unknown answers neither, and in both cases the safe
    // move is the same one: keep running, on the deterministic surface.
    const r = world({ facts: {} })
    const res = await run(wide, { ticket: 'x' }, r)
    expect(res.widened).toBe(false)
    expect(r.requests[0]?.messages[0]?.content).toContain('Judge the reported outcome')
  })

  it('does not widen unless every endpoint in the pool has earned it', async () => {
    const r = world({ endpoints: ['spark', 'spark-2'], facts: { spark: { 'tool-select': true } } })
    expect((await run(wide, { ticket: 'x' }, r)).widened).toBe(false)
  })

  it('a harness with no widen declaration never widens', async () => {
    expect((await run(judge, { ticket: 'x' }, world({ facts: { spark: { 'tool-select': true } } }))).widened).toBe(false)
  })

  // ── Provenance: only a fact THIS PLATFORM MEASURED may widen ───────────────
  //
  // The floor one branch up refuses on anything that is not `learned`, so a
  // human or a catalog saying "this model cannot do JSON" is grounds to stop.
  // Widening is the other direction — it HANDS A MODEL MORE AUTHORITY, and for
  // the Inbox command harness that authority is choosing which action to take on
  // somebody's ticket — so it takes a measurement and nothing else. The
  // asymmetry is deliberate and a reader will assume it is a bug; these are the
  // assertions that say otherwise.

  it('IGNORES a declared fact: a vendor cannot widen the Inbox by claiming a capability', async () => {
    // The day anything imports a model catalog as `declared: true` — which is
    // exactly what `defaultDeps.advertises` already reads for the vision probe —
    // this is what stops a marketing claim from arming the widened surface on
    // every install that synced it.
    const r = world({ facts: { spark: { 'tool-select': { value: true, source: 'declared' } } } })
    const res = await run(wide, { ticket: 'x' }, r)
    expect(res.widened).toBe(false)
    expect(r.requests[0]?.messages[0]?.content).toContain('Judge the reported outcome')
    // Not a refusal either: an unmeasured model still runs, on the narrow
    // surface, which is the whole "decent on a 14B" half of the requirement.
    expect(res.value).not.toBeNull()
  })

  it('ignores a learned fact for the same reason, and the gateway only ever writes false anyway', async () => {
    const r = world({ facts: { spark: { 'tool-select': { value: true, source: 'learned' } } } })
    expect((await run(wide, { ticket: 'x' }, r)).widened).toBe(false)
  })

  it('HONORS a probe fact — the one source that is Talaria measuring the model itself', async () => {
    const r = world({ facts: { spark: { 'tool-select': { value: true, source: 'probe' } } } })
    const res = await run(wide, { ticket: 'x' }, r)
    expect(res.widened).toBe(true)
    expect(r.runs[0]?.widened).toBe(true)
  })

  it('needs the whole pool measured, not one probe and one claim', async () => {
    const r = world({
      endpoints: ['spark', 'spark-2'],
      facts: {
        'spark:pl-main': { 'tool-select': { value: true, source: 'probe' } },
        'spark-2:pl-main': { 'tool-select': { value: true, source: 'declared' } },
      },
    })
    expect((await run(wide, { ticket: 'x' }, r)).widened).toBe(false)
  })
})

// ── Tool definitions on the request (transport.ts's sixth slot) ──────────────

describe('a harness that OFFERS tools', () => {
  const WEATHER = { name: 'get_weather', description: 'Current weather for a city.', parameters: { type: 'object', properties: { city: { type: 'string' } } } }

  it('carries the definitions to the transport, distinct from the tool POLICY', async () => {
    // The two are different sentences about different tools: the policy governs
    // the model's OWN loop (a persona's, running inside the agent), and these
    // are tools we hand over and watch being called. `tool-select` — the probe
    // that gates the Inbox widening — is unrunnable without this slot.
    const r = world()
    await run({ ...judge, toolDefs: [WEATHER] }, { ticket: 'x' }, r)
    expect(r.requests[0]?.toolDefs).toEqual([WEATHER])
    expect(r.requests[0]?.tools).toBeUndefined()
  })

  it('sends no slot at all when the harness declares none, so a transport cannot see an empty offer', async () => {
    const r = world()
    await run(judge, { ticket: 'x' }, r)
    expect(r.requests[0]?.toolDefs).toBeUndefined()
  })

  it('guards a reported tool call as NAMES WITHOUT RESULTS — a call is not a result', async () => {
    // Nothing executed the call, so there is no result text to ground a citation
    // against. Counting the name as a backing tool with an empty results record
    // would make `ungrounded_ref` fire on every id in the reply; the fleet path
    // has always been handled this way and the gateway now joins it.
    const withCall: TransportReply = {
      kind: 'gateway',
      text: 'Filed 3f0c8a52-6b1d-4a7e-9d21-0f8e5c4b2a91 for you.',
      toolNames: ['create_ticket'],
      toolCalls: [{ name: 'create_ticket', args: '{"title":"x"}' }],
      usage: null,
      contractDropped: false,
    }
    const r = world({ replies: [withCall] })
    const res = await run({ ...titler, toolDefs: [WEATHER] }, { transcript: 'x' }, r)
    expect(res.findings.map((f) => f.check)).not.toContain('ungrounded_ref')
  })
})

// ── Fleet personas ───────────────────────────────────────────────────────────

describe('capability records on a fleet persona', () => {
  // A persona is not a gateway catalog model: `routingFor` answers with no
  // endpoints, so before the persona lookup existed the key list was empty on
  // every persona run — which made widening a structural impossibility there.
  // The marquee widening case in the product (the Inbox command harness handing
  // a capable model the item's full action list, audit 1.8) runs on the owner's
  // PERSONAL ASSISTANT, which is a persona, so "excel with larger models" could
  // never fire on the path it was written for.
  const wide: HarnessDefinition<{ ticket: string }, Verdict> = {
    ...judge,
    widen: { requires: ['tool-select'], note: 'Models that pick the right tool get the full action list.' },
  }
  const onPersona = (id: string): World['model'] => ({ model: id, step: 'pin' })

  /** Penny: a 14B local main, a frontier "opus" tier, no fallbacks. */
  const penny: PersonaRow = {
    agent: 'assistant-operations',
    config: {
      main: { endpoint: 'spark', model: 'qwen3-14b' },
      aliases: [{ name: 'opus', endpoint: 'anthropic', model: 'claude-opus-4' }],
    },
  }

  it('inherits the probe of the model actually behind it, and widens', async () => {
    const r = world({ model: onPersona('assistant-operations'), personas: [penny], facts: { spark: { 'tool-select': true } } })
    const res = await run(wide, { ticket: 'x' }, r)

    expect(res.widened).toBe(true)
    expect(r.requests[0]?.messages[0]?.content).toContain('cite the diff')
    expect(r.runs[0]?.widened).toBe(true)
  })

  it('does not widen on a persona whose backing model nobody has probed — but runs', async () => {
    const r = world({ model: onPersona('assistant-operations'), personas: [penny], facts: {} })
    const res = await run(wide, { ticket: 'x' }, r)

    expect(res.widened).toBe(false)
    expect(r.requests).toHaveLength(1)
    expect(res.value).not.toBeNull()
  })

  it('resolves the TIER being called, not the agent main', async () => {
    // "assistant-operations-opus" is a different, larger model than "assistant-operations"
    // even though one id is a prefix of the other. Crediting main's probe to the
    // tier would widen on a fact about something else entirely.
    const facts = { anthropic: { 'tool-select': true } }
    const base = world({ model: onPersona('assistant-operations'), personas: [penny], facts })
    const tier = world({ model: onPersona('assistant-operations-opus'), personas: [penny], facts })

    expect((await run(wide, { ticket: 'x' }, base)).widened).toBe(false)
    expect((await run(wide, { ticket: 'x' }, tier)).widened).toBe(true)
  })

  it('yields NO keys for a tier the agent does not have', async () => {
    // Inheriting the wrong model's capabilities is worse than inheriting none:
    // unknown is safe in both directions by design, and a wrong fact is not.
    const r = world({ model: onPersona('assistant-operations-turbo'), personas: [penny], facts: { spark: { 'tool-select': true } } })
    const res = await run(wide, { ticket: 'x' }, r)

    expect(res.widened).toBe(false)
    expect(r.requests).toHaveLength(1) // unknown still RUNS
  })

  it('an agent whose own id looks like another agent tier keeps its own config', async () => {
    const impostor: PersonaRow = { agent: 'assistant-operations-opus', config: { main: { endpoint: 'spark', model: 'qwen3-14b' } } }
    const r = world({
      model: onPersona('assistant-operations-opus'),
      personas: [penny, impostor],
      facts: { anthropic: { 'tool-select': true } },
    })
    // Penny's "opus" alias points at anthropic, but this id belongs to a real
    // agent of its own whose main is the 14B — the agent's own config wins.
    expect((await run(wide, { ticket: 'x' }, r)).widened).toBe(false)
  })

  it('keeps the pool unanimous across a persona backed by a fallback chain', async () => {
    // Hermes moves to a fallback provider when the main errors, so which model
    // answers is genuinely unknowable in advance — the same situation as a
    // gateway model served by several endpoints, and it gets the same answer.
    const withFallback: PersonaRow = {
      agent: 'engineer-engineering',
      config: {
        main: { endpoint: 'spark', model: 'qwen3-14b' },
        fallbacks: [{ endpoint: 'thunder', model: 'llama3-8b' }],
      },
    }
    const half = world({ model: onPersona('engineer-engineering'), personas: [withFallback], facts: { spark: { 'tool-select': true } } })
    const all = world({
      model: onPersona('engineer-engineering'),
      personas: [withFallback],
      facts: { spark: { 'tool-select': true }, thunder: { 'tool-select': true } },
    })

    expect((await run(wide, { ticket: 'x' }, half)).widened).toBe(false)
    expect((await run(wide, { ticket: 'x' }, all)).widened).toBe(true)
  })

  it('refuses below the floor when EVERY model behind the persona is known to fail it', async () => {
    // The keys feed step 2 as well as step 3: a persona backed by a model known
    // to be unable to return JSON is a judge that would escalate everything.
    const r = world({ model: onPersona('assistant-operations'), personas: [penny], facts: { spark: { json: false } } })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(r.requests).toHaveLength(0)
    expect(res.error).toContain('json')
  })

  it('does not refuse when one member of a persona pool is merely unprobed', async () => {
    const withFallback: PersonaRow = {
      agent: 'engineer-engineering',
      config: { main: { endpoint: 'spark', model: 'qwen3-14b' }, fallbacks: [{ endpoint: 'thunder', model: 'llama3-8b' }] },
    }
    const r = world({ model: onPersona('engineer-engineering'), personas: [withFallback], facts: { spark: { json: false } } })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(r.requests).toHaveLength(1)
    expect(res.value).not.toBeNull()
  })

  it('carries on when the config lookup throws', async () => {
    // Resolving a persona hits the database. That lookup exists to make a run
    // BETTER; it is not a precondition for running one, and a database blip must
    // never be the reason a harness fails.
    const r = world({ model: onPersona('assistant-operations'), personasThrow: true })
    const res = await run(wide, { ticket: 'x' }, r)

    expect(res.widened).toBe(false)
    expect(res.value).toEqual({ verdict: 'pass', summary: 'looks right' })
    expect(res.error).toBeUndefined()
  })

  it('never consults the persona index for a model the gateway already routes', async () => {
    // The gateway's own endpoints are the authoritative answer when there are
    // any; the persona lookup is strictly the empty-handed case.
    const r = world({ endpoints: ['spark'], personasThrow: true, facts: { spark: { 'tool-select': true } } })
    expect((await run(wide, { ticket: 'x' }, r)).widened).toBe(true)
  })
})

// ── What a red cell says ─────────────────────────────────────────────────────

describe('the failure text on the run row', () => {
  it('records the sentence behind a failed contract', async () => {
    const r = world({ replies: ['not json', 'still not json'] })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(r.runs[0]?.error).toBe(res.error)
    expect(r.runs[0]?.error).toContain('no JSON object or array was found')
  })

  it('leaves the column null on a run that worked', async () => {
    const r = world()
    await run(judge, { ticket: 'x' }, r)
    expect(r.runs[0]?.error).toBeNull()
  })

  it('scrubs a credential the parser quoted back out of the failure text', async () => {
    // Parser errors quote the model's own rejected value, so this string is
    // model output too — and unlike `raw` it is kept forever. Redaction happens
    // BEFORE the length bound, because slicing first can cut a credential in
    // half so that no pattern matches the tail.
    const r = world({ replies: [`{"verdict":"AKIAIOSFODNN7EXAMPLE","summary":"x"}`, 'nope'] })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(res.error).toContain('AKIAIOSFODNN7EXAMPLE') // the live result is untouched
    expect(r.runs[0]?.error).not.toContain('AKIAIOSFODNN7EXAMPLE')
    expect(r.runs[0]?.error).toContain('[redacted AWS access key]')
  })

  it('bounds a pathological failure sentence', async () => {
    const r = world({ replies: [`{"verdict":"${'x'.repeat(5_000)}","summary":"y"}`, 'nope'] })
    await run(judge, { ticket: 'x' }, r)
    expect((r.runs[0]?.error ?? '').length).toBeLessThanOrEqual(1_000)
  })
})

// ── Guardrails (audit 1.5) ───────────────────────────────────────────────────

describe('the guard pass', () => {
  const leaky = '{"verdict":"pass","summary":"deployed with key AKIAIOSFODNN7EXAMPLE"}'

  it('records findings against the harness output', async () => {
    const r = world({ replies: [leaky] })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(res.findings.map((f) => f.check)).toContain('secret_leak')
    expect(r.recorded.map((f) => f.check)).toContain('secret_leak')
    expect(r.runs[0]?.findings).toBe(1)
    // Observe mode never touches the value — that is the default posture.
    expect(res.value?.summary).toContain('AKIAIOSFODNN7EXAMPLE')
  })

  it('redacts the value when the harness asked for it, and re-applies the contract', async () => {
    const r = world({ replies: [leaky] })
    const res = await run({ ...judge, guard: { redact: true } }, { ticket: 'x' }, r)
    expect(res.value?.summary).toBe('deployed with key [redacted AWS access key]')
    expect(res.schemaValid).toBe(true)
  })

  it('narrows the registry to the rules a harness declared', async () => {
    const r = world({ replies: [leaky] })
    const res = await run({ ...judge, guard: { rules: ['zero_tool_claim'] } }, { ticket: 'x' }, r)
    expect(res.findings).toEqual([])
    expect(r.recorded).toEqual([])
  })

  it('runs nothing when the guard is off', async () => {
    const r = world({ replies: [leaky], guardMode: 'off' })
    const res = await run(judge, { ticket: 'x' }, r)
    expect(res.findings).toEqual([])
    expect(r.recorded).toEqual([])
  })

  it('skips the rules a fleet transport cannot honestly supply', async () => {
    // The persona's tool loop runs inside the agent, so the stream gives tool
    // NAMES and nothing else. A rule that needs tool RESULTS is skipped rather
    // than run on missing data — guardChatReply's precedent, and the reason
    // `ungrounded_ref` does not fire on this uncited UUID.
    const fleetReply: TransportReply = {
      kind: 'fleet',
      text: 'Created the ticket 3f0c8a52-6b1d-4a7e-9d21-0f8e5c4b2a91.',
      toolNames: ['think'],
      usage: null,
      contractDropped: false,
    }
    const r = world({ replies: [fleetReply] })
    const res = await run(titler, { transcript: 'x' }, r)

    expect(res.findings.map((f) => f.check)).not.toContain('ungrounded_ref')
    // 'think' is not a backing tool (guardrails.ts owns that list), so the
    // zero-tool claim still stands.
    expect(res.findings.map((f) => f.check)).toContain('zero_tool_claim')
  })
})

// ── Grounding the guard against the run's own input ──────────────────────────
//
// `groundingTextOf` shipped with `agent-writes.ts` and `guardCompletion` wired
// to it and THIS RUNNER NOT — so the one path that guards 23 harnesses was the
// one path that grounded nothing, and it is the path that also REDACTS THE
// VALUE. Both halves of `pii_leak`'s `groundable: 'finding+redaction'` were
// therefore violated at once on every harness with `guard: { redact: true }`,
// which is 20 of the 23.
describe('grounding the guard pass against the input', () => {
  // Luhn-valid, so `pii_leak` reads it as a payment card. It is an ORDER NUMBER
  // in the ticket, which is the whole measured problem: business identifiers
  // share the shapes the detectors match.
  const ORDER = '4242424242424242'
  const quoted = `{"verdict":"pass","summary":"refunded order ${ORDER} as the customer asked"}`

  /** The ticket text goes in the SYSTEM message. That is deliberate and it is
   *  what separates this from the pre-grounding behavior: `userMessage` alone
   *  never held the rendered material, so a check that grounded against it
   *  would still flag every one of these. */
  const support = defineHarness<{ ticket: string }, Verdict>({
    ...judge,
    id: 'support-judge',
    render: (input) => [
      { role: 'system', content: `Judge the outcome. Ticket: ${input.ticket}` },
      { role: 'user', content: 'Did the agent resolve it?' },
    ],
    guard: { redact: true },
  })

  it('files nothing against the model for an identifier that was in its own prompt', async () => {
    const r = world({ replies: [quoted] })
    const res = await run(support, { ticket: `customer disputes order ${ORDER}` }, r)

    expect(res.findings).toEqual([])
    expect(r.recorded).toEqual([])
    // The guard rate the fitness page reads has to agree with `guard_findings`.
    expect(r.runs[0]?.findings).toBe(0)
  })

  it('and does not rewrite it out of the value it persists', async () => {
    // The worse half of the bug: the finding is out-of-band telemetry, the
    // value is the artifact a human reads. A distillation in which the order
    // number has become `[redacted card number]` disagrees with the chat it
    // summarized.
    const r = world({ replies: [quoted] })
    const res = await run(support, { ticket: `customer disputes order ${ORDER}` }, r)
    expect(res.value?.summary).toContain(ORDER)
  })

  it('still flags AND redacts a number that appears nowhere in the prompt', async () => {
    const r = world({ replies: [quoted] })
    const res = await run(support, { ticket: 'customer disputes a delivery' }, r)

    expect(res.findings.map((f) => f.check)).toContain('pii_leak')
    expect(res.value?.summary).toContain('[redacted card number]')
    expect(r.runs[0]?.findings).toBe(1)
  })

  it('redacts a credential the operator pasted, and still blames nobody for it', async () => {
    // `secret_leak` is `groundable: 'finding'` — the other half of the split.
    // The key really is a key, so the copy Talaria writes down loses it; the
    // model did not invent it, so no row says it did.
    const key = 'AKIAIOSFODNN7EXAMPLE'
    const r = world({ replies: [`{"verdict":"pass","summary":"rotated ${key} as instructed"}`] })
    const res = await run(support, { ticket: `the leaked key is ${key}` }, r)

    expect(res.findings.map((f) => [f.check, f.grounded])).toEqual([['secret_leak', true]])
    expect(res.value?.summary).toBe('rotated [redacted AWS access key] as instructed')
    expect(r.runs[0]?.findings).toBe(0)
  })

  it('spends the repair turn on a reply flagged only for quoting the prompt back', async () => {
    // The repair gate reads the same material. Grounding it is what stops the
    // run's one second chance being burned on a finding that will never be
    // filed: the reply below is malformed AND carries the order number, and
    // before this the gate refused it and the harness produced nothing.
    const r = world({ replies: [`sure - order ${ORDER}: {"verdict":"maybe"`, quoted] })
    const res = await run(support, { ticket: `customer disputes order ${ORDER}` }, r)

    expect(res.repairs).toBe(1)
    expect(res.schemaValid).toBe(true)
    expect(res.value?.summary).toContain(ORDER)
  })
})

// ── Everything that can go wrong before a model is reached ───────────────────

describe('when there is no model', () => {
  it('returns a result rather than throwing, and still writes a row', async () => {
    const r = world({ model: null })
    const res = await run(judge, { ticket: 'x' }, r)

    expect(res.value).toBeNull()
    expect(res.model).toBeNull()
    expect(res.error).toContain('no model available')
    expect(r.runs[0]?.model).toBeNull()
  })

  it('turns a transport failure into a result, not an exception', async () => {
    const r = world()
    const res = await runHarness(judge, { ticket: 'x' }, {
      caller: 'test:harness',
      deps: {
        ...r.deps,
        transport: async () => {
          throw new Error('gateway completion 503')
        },
      },
    })
    expect(res.value).toBeNull()
    expect(res.error).toContain('503')
    expect(r.runs).toHaveLength(1)
  })

  it('takes an explicit model pin without consulting the chain', async () => {
    // How the fitness suite replays a harness against a candidate model.
    const r = world({ model: null })
    const res = await runHarness(judge, { ticket: 'x' }, { caller: 'fitness', model: 'candidate-14b', deps: r.deps })
    expect(res.model).toBe('candidate-14b')
    expect(res.step).toBeNull()
    expect(r.requests[0]?.model).toBe('candidate-14b')
  })

  it('fails cleanly when a render produces nothing', async () => {
    const r = world()
    const empty: HarnessDefinition<{ ticket: string }, Verdict> = { ...judge, render: (): Message[] => [] }
    const res = await run(empty, { ticket: 'x' }, r)
    expect(res.error).toContain('rendered no messages')
    expect(r.requests).toHaveLength(0)
  })

  // `render` and `clean` are the two pieces of HARNESS-AUTHOR code the runner
  // executes, so they are the two places an exception can escape a function
  // whose entire promise is that a bad model — or a bad harness — produces a
  // result. Both are contract failures, spelled the same way as any other.
  it('a render that throws synchronously is a result, not an exception', async () => {
    const r = world()
    const boom: HarnessDefinition<{ ticket: string }, Verdict> = {
      ...judge,
      render: (): Message[] => {
        throw new TypeError("Cannot read properties of undefined (reading 'title')")
      },
    }
    const res = await run(boom, { ticket: 'x' }, r)
    expect(res.error).toContain('rendered no messages')
    expect(r.runs).toHaveLength(1)
  })

  it('a clean step that throws fails the contract instead of escaping', async () => {
    const r = world({ replies: ['a perfectly good title'] })
    const boom: HarnessDefinition<{ transcript: string }, string> = {
      ...titler,
      output: {
        kind: 'text',
        clean: (): string => {
          throw new RangeError('nope')
        },
      },
    }
    const res = await run(boom, { transcript: 'x' }, r)
    expect(res.value).toBeNull()
    expect(res.error).toContain('clean step threw')
    expect(r.runs).toHaveLength(1)
  })
})

// ── The runner transport gap: one feature that wore five coats ───────────────
//
// `work-dispatch.ts`, `briefing.ts`, `outreach.ts`, `plan-persona-turn.ts` and
// `routes/api/muse.ts` each hand-wrote a persona transport because
// `TransportRequest` had no slot for what they needed. All five are deleted now
// and every assertion below is one of the slots that replaced them — which is
// the point of testing them HERE rather than through the four callers: these
// cases are what makes the deletion safe to keep, and they would still hold if
// every one of those callers were rewritten tomorrow.
//
// Three of the five shims silently dropped `req.temperature` / `req.jsonMode`
// on the way, which is why the last case here is about a field that CANNOT be
// honored failing rather than vanishing.

describe('tool passthrough', () => {
  it("tells the transport when the model may use its OWN tools", async () => {
    // The work session, the outreach check-in and the briefing follow-up are
    // tool loops. `tools: []` / `tool_choice: 'none'` does not weaken them — it
    // disarms the agent and then trips `zero_tool_claim` for calling no tool.
    const r = world({ replies: ['picked it up, running the tests now'] })
    const looping: HarnessDefinition<{ transcript: string }, string> = { ...titler, tools: 'own' }
    await run(looping, { transcript: 'x' }, r)
    expect(r.requests[0]?.tools).toBe('own')
  })

  it('leaves it absent for every ordinary harness, which the transport reads as none', async () => {
    const r = world()
    await run(judge, { ticket: 'x' }, r)
    expect(r.requests[0]?.tools).toBeUndefined()
  })

  it('carries the hold deadline a slow persona needs', async () => {
    // An agent restarting under a config propagation refuses connections for
    // tens of seconds; `proxyChat` waits two minutes by default and a work
    // session wants ten. It was the third reason `sessionTransport` existed.
    const r = world({ replies: ['ok'] })
    const patient: HarnessDefinition<{ transcript: string }, string> = { ...titler, holdMs: 600_000 }
    await run(patient, { transcript: 'x' }, r)
    expect(r.requests[0]?.holdMs).toBe(600_000)
  })

  it('FAILS rather than quietly dropping tools a transport cannot serve', async () => {
    // The rule the whole request type is built on. Three of the five shims
    // silently dropped `temperature` and `jsonMode`; a field that cannot be
    // honored has to fail the call, because a disarmed tool-loop harness does
    // not read as broken — it reads as an agent that decided not to act, and
    // then gets flagged `zero_tool_claim` for saying it did something.
    //
    // Both refusals are asserted before either touches the network: the throw is
    // the first statement in each transport.
    const req: TransportRequest = { model: 'qwen3-14b', messages: [{ role: 'user', content: 'x' }], jsonMode: false, tools: 'own', caller: 't' }
    await expect(gatewayTransport(req)).rejects.toThrow(/served by the ORG GATEWAY/)
    await expect(gatewayStream(req, () => {})).rejects.toThrow(/served by the ORG GATEWAY/)
  })

  it('still sends temperature and jsonMode alongside them', async () => {
    // THE REGRESSION THE SHIMS SHIPPED, asserted from the other side: a harness
    // that declares `temperature: 0` and runs at the provider's default is a
    // harness whose declaration is decorative, and nothing said so.
    const r = world()
    const looping: HarnessDefinition<{ ticket: string }, Verdict> = { ...judge, tools: 'own' }
    await run(looping, { ticket: 'x' }, r)
    expect(r.requests[0]).toMatchObject({ tools: 'own', temperature: 0, jsonMode: true })
  })
})

describe('ledger attribution', () => {
  it('defaults to the attribution a harness turn has always had', async () => {
    const r = world()
    await run(judge, { ticket: 'x' }, r)
    expect(r.requests[0]?.ledger).toEqual({ agentModel: 'pl-main', source: 'chat', refId: null, taskId: null, tier: null })
  })

  it("restores research's own source and run id", async () => {
    // The persona stages metered as `source: 'chat'` with no refId after the
    // port, so a run's cost stopped being answerable at all.
    const r = world()
    const res = await runHarness(judge, { ticket: 'x' }, {
      caller: 'research:run-9',
      ledger: { source: 'research', refId: 'run-9' },
      deps: r.deps,
    })
    expect(res.value).not.toBeNull()
    expect(r.requests[0]?.ledger).toEqual({ agentModel: 'pl-main', source: 'research', refId: 'run-9', taskId: null, tier: null })
  })

  it("carries the work session's taskId, without which the spend misses the ticket", async () => {
    const r = world({ replies: ['on it'] })
    await runHarness(titler, { transcript: 'x' }, {
      caller: 'ticket:t-41',
      model: 'engineer-engineering',
      ledger: { source: 'chat', refId: 't-41', taskId: 't-41' },
      deps: r.deps,
    })
    expect(r.requests[0]?.ledger).toMatchObject({ agentModel: 'engineer-engineering', refId: 't-41', taskId: 't-41' })
  })
})

describe('routing a persona TIER', () => {
  const onTier = { caller: 'plan:c-1', model: 'engineer-engineering', tier: 'opus' as const }

  it('calls the tier id and attributes the spend to the base agent', async () => {
    // `recordUsage` prices a row by finding `agent_defs.model = agentModel` and
    // then the alias named by `tier`. Hand it the routed id with a null tier and
    // BOTH lookups miss: the row lands on an agent that does not exist, with no
    // endpoint class, which means no price. A tier draft becomes free.
    const r = world({ replies: ['drafted'] })
    const res = await runHarness(titler, { transcript: 'x' }, { ...onTier, deps: r.deps })

    expect(r.requests[0]?.model).toBe('engineer-engineering-opus')
    expect(r.requests[0]?.ledger).toMatchObject({ agentModel: 'engineer-engineering', tier: 'opus' })
    // The RESULT names the id that answered, because that is the model the
    // fitness matrix is scoring.
    expect(res.model).toBe('engineer-engineering-opus')
    expect(r.runs[0]?.model).toBe('engineer-engineering-opus')
  })

  it('asks the capability index about the TIER, not the agent main', async () => {
    // "engineer-engineering-opus" is a different, usually larger model than
    // "engineer-engineering"; crediting main's probe to the tier would widen on a fact
    // about something else. `persona.ts` already resolves it — the runner just
    // has to ask about the right id.
    const asked: string[] = []
    const r = world({ replies: ['drafted'] })
    await runHarness(titler, { transcript: 'x' }, {
      ...onTier,
      deps: { ...r.deps, routing: async () => ({ endpoints: [], upstreamModel: '' }), personaKeys: async (m) => (asked.push(m), []) },
    })
    expect(asked).toEqual(['engineer-engineering-opus'])
  })
})

describe('a caller that ran the chain itself', () => {
  // `routes/api/muse.ts` must know the model BEFORE it opens the stream —
  // `x-muse-model` is a header, and "nothing routes" has to be a 400 rather than
  // a stream that opens and closes empty. So it resolves the chain and hands the
  // answer over. `ctx.step` is what stops that from silently costing the
  // fitness page its `chain_step` column: an install limping along on
  // 'first-routable' for a month is a real finding, and pinning alone erases it.

  it('keeps the step on the run row', async () => {
    const r = world({ replies: ['drafted'] })
    await runHarness(titler, { transcript: 'x' }, { caller: 'muse:test', model: 'pl-fast', step: 'first-routable', deps: r.deps })
    expect(r.runs[0]).toMatchObject({ model: 'pl-fast', step: 'first-routable' })
  })

  it('records NO step for a caller that genuinely pinned', async () => {
    // The fitness suite names a candidate model; there was no chain, so
    // inventing a step would put a fabricated fallback in the operator's data.
    const r = world({ replies: ['drafted'] })
    await runHarness(titler, { transcript: 'x' }, { caller: 'fitness:test', model: 'pl-fast', deps: r.deps })
    expect(r.runs[0]).toMatchObject({ model: 'pl-fast', step: null })
  })

  it('ignores a step offered without a model, rather than reporting a chain that never ran', async () => {
    const r = world({ replies: ['drafted'] })
    await runHarness(titler, { transcript: 'x' }, { caller: 'muse:test', step: 'first-routable', deps: r.deps })
    // `world` resolves to pl-main via 'pin'; the caller's stray step loses.
    expect(r.runs[0]).toMatchObject({ model: 'pl-main', step: 'pin' })
  })
})

// ── Streaming (audit 1.5, the Muse row and the briefing panel) ───────────────

describe('runHarnessStreamed', () => {
  /** A transport that pumps three deltas and never assembles the text itself —
   *  the honest shape of a route that pipes chunks straight into a Response. */
  const pump =
    (...deltas: string[]) =>
    async (_req: TransportRequest, emit: (d: string) => void): Promise<TransportReply> => {
      for (const d of deltas) emit(d)
      return { kind: 'gateway', text: '', toolNames: [], usage: null, contractDropped: false }
    }

  it('hands every delta on as it arrives and still guards the whole reply', async () => {
    const r = world()
    const chunks: string[] = []
    const res = await runHarnessStreamed(titler, { transcript: 'x' }, { caller: 'muse', deps: r.deps }, {
      stream: pump('Migrating ', 'the ledger ', 'to Postgres'),
      onDelta: (d) => chunks.push(d),
    })

    expect(chunks).toEqual(['Migrating ', 'the ledger ', 'to Postgres'])
    // The reply resolved with `text: ''` — the accumulated deltas ARE the reply,
    // which is what lets a route pump into a browser and assemble nothing.
    expect(res.value).toBe('Migrating the ledger to Postgres')
    expect(res.schemaValid).toBe(true)
    expect(r.runs).toHaveLength(1)
  })

  it('redacts the VALUE it returns, though the bytes it relayed are gone', async () => {
    // guardrails.ts cleans "what Talaria persists or hasn't yet relayed". The
    // stream already showed the original; a surface that must scrub what it
    // relays does that in `onDelta`, which is the only place that can work.
    const r = world()
    const chunks: string[] = []
    const res = await runHarnessStreamed(
      { ...titler, guard: { redact: true } },
      { transcript: 'x' },
      { caller: 'muse', deps: r.deps },
      { stream: pump('deployed with key ', 'AKIAIOSFODNN7EXAMPLE'), onDelta: (d) => chunks.push(d) },
    )

    expect(res.findings.map((f) => f.check)).toContain('secret_leak')
    expect(res.value).toBe('deployed with key [redacted AWS access key]')
    expect(chunks.join('')).toContain('AKIAIOSFODNN7EXAMPLE')
  })

  it('never repairs — the first answer already reached the screen', async () => {
    // Repairing would stream one document and hand back another. The contract
    // failure is recorded honestly instead, which is the number the fitness page
    // needs about a model on a streaming surface.
    const r = world()
    let calls = 0
    const res = await runHarnessStreamed(judge, { ticket: 'x' }, { caller: 'muse', deps: r.deps }, {
      stream: async (_req, emit) => {
        calls++
        emit('here is my verdict: maybe')
        return { kind: 'gateway', text: '', toolNames: [], usage: null, contractDropped: false }
      },
    })

    expect(calls).toBe(1)
    expect(res.repairs).toBe(0)
    expect(res.value).toBeNull()
    expect(r.runs[0]?.schemaValid).toBe(false)
  })

  it('keeps the partial reply when the stream dies mid-flight', async () => {
    // `raw` is what the fitness drill-down shows behind a red cell, and the
    // failure an operator most wants to interrogate is exactly this one. The
    // accumulator was declared INSIDE the attempt loop, so a transport that
    // threw took the relayed bytes with it and the row came back with nothing
    // behind it — while the comment on that catch claimed the opposite.
    const r = world()
    const res = await runHarnessStreamed(titler, { transcript: 'x' }, { caller: 'muse', deps: r.deps }, {
      stream: async (_req, emit) => {
        emit('Migrating the ledger ')
        emit('to Postg')
        throw new Error('socket hang up')
      },
    })

    expect(res.value).toBeNull()
    expect(res.raw).toBe('Migrating the ledger to Postg')
    expect(res.error).toContain('socket hang up')
    expect(r.runs).toHaveLength(1)
  })

  it('carries the tools, ledger and tier slots exactly as a blocking run does', async () => {
    const r = world()
    let seen: TransportRequest | null = null
    await runHarnessStreamed(
      { ...titler, tools: 'own' },
      { transcript: 'x' },
      { caller: 'briefer:chat', model: 'assistant-operations', ledger: { refId: 'c-3' }, deps: r.deps },
      {
        stream: async (req, emit) => {
          seen = req
          emit('here is what needs you')
          return { kind: 'fleet', text: '', toolNames: ['get_ticket'], usage: null, contractDropped: false }
        },
      },
    )
    expect(seen).toMatchObject({ model: 'assistant-operations', tools: 'own', ledger: { refId: 'c-3', agentModel: 'assistant-operations' } })
  })
})

// ── The grounding hook (audit 1.5, the rule that could never fire) ───────────

describe('the grounding hook', () => {
  const HIT = 'https://talaria.internal/research/sources/aa11bb22 — the vendor published a SOC 2 Type II'

  /** A synthesis-shaped harness: its input carries the search hits, which ARE
   *  the tool results for the turn. */
  const synthesis = defineHarness<{ notes: string; failed: boolean }, string>({
    id: 'research-synthesis-test',
    label: 'Synthesis',
    job: 'Writes the report from the findings.',
    requires: [],
    floor: { capabilities: [], refuseBelow: false, note: 'Runs on anything.' },
    model: { pin: 'titler' },
    render: () => [{ role: 'user', content: 'write it up' }],
    output: { kind: 'text' },
    onFailure: 'null',
    ground: (input) => ({ tools: ['research_search'], results: input.notes, errored: input.failed }),
    guard: { rules: ['ungrounded_ref', 'fabricated_outage'] },
  })

  const policed = (mode: GuardConfig['mode'] = 'observe') => ({
    guardConfig: async (): Promise<GuardConfig> => ({ ...DEFAULT_CONFIG, mode, policedHosts: ['talaria.internal'] }),
  })

  it('FIRES ungrounded_ref on a link no search result contained', async () => {
    // The definitive research failure mode — the persona's soul and memory
    // bleeding an internal link into a document a human will trust because it
    // looks cited — and until this hook existed the rule could not run on a
    // single harness in the product.
    const r = world({ replies: ['See https://talaria.internal/tickets/9f9f9f9f for the detail.'] })
    const res = await runHarness(synthesis, { notes: HIT, failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })

    expect(res.findings.map((f) => f.check)).toContain('ungrounded_ref')
    expect(r.recorded.map((f) => f.check)).toContain('ungrounded_ref')
  })

  it('does NOT fire on a link the search results really returned', async () => {
    const r = world({ replies: [`The vendor's report is at https://talaria.internal/research/sources/aa11bb22 [1].`] })
    const res = await runHarness(synthesis, { notes: HIT, failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(res.findings).toEqual([])
  })

  it('grounds a PERSONA turn too, because the material is external to its tool loop', async () => {
    // The synthesis stage runs on the requesting agent's own persona, whose
    // stream reports tool names only. The search hits are not that persona's
    // tools — they are the pipeline's — so `ground` overrides the fleet branch
    // rather than deferring to it.
    const fleet: TransportReply = {
      kind: 'fleet',
      text: 'See https://talaria.internal/tickets/9f9f9f9f.',
      toolNames: [],
      usage: null,
      contractDropped: false,
    }
    const r = world({ replies: [fleet] })
    const res = await runHarness(synthesis, { notes: HIT, failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(res.findings.map((f) => f.check)).toContain('ungrounded_ref')
  })

  it('lets a real stage failure ground an outage claim', async () => {
    const claim = 'One angle could not be researched: the search endpoint was unreachable.'
    const r = world({ replies: [claim] })
    const honest = await runHarness(synthesis, { notes: HIT, failed: true }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(honest.findings.map((f) => f.check)).not.toContain('fabricated_outage')

    const invented = await runHarness(synthesis, { notes: HIT, failed: false }, { caller: 'research:r1', deps: { ...world({ replies: [claim] }).deps, ...policed() } })
    expect(invented.findings.map((f) => f.check)).toContain('fabricated_outage')
  })

  it("SKIPS fabricated_outage when the harness cannot say whether anything errored", async () => {
    // `errored: null` is the difference between a rule that is quiet and a rule
    // that is wrong. Claiming "nothing errored" on material that does not record
    // errors would flag every honest report of a failure.
    const cagey: HarnessDefinition<{ notes: string; failed: boolean }, string> = {
      ...synthesis,
      ground: (input) => ({ tools: ['research_search'], results: input.notes, errored: null }),
    }
    const r = world({ replies: ['One angle could not be researched: the search endpoint was unreachable.'] })
    const res = await runHarness(cagey, { notes: HIT, failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(res.findings.map((f) => f.check)).not.toContain('fabricated_outage')
    // …and the grounding it CAN supply still works.
    expect(res.findings.map((f) => f.check)).not.toContain('ungrounded_ref')
  })

  // ── The hook must make honesty expressible, not optimism the default ───────

  it('a harness with NO ground still skips ungrounded_ref', async () => {
    const r = world({ replies: ['See https://talaria.internal/tickets/9f9f9f9f.'] })
    const res = await runHarness(
      { ...synthesis, ground: undefined },
      { notes: HIT, failed: false },
      { caller: 'research:r1', deps: { ...r.deps, ...policed() } },
    )
    expect(res.findings).toEqual([])
  })

  it('a ground that returns NOTHING TO GROUND AGAINST is treated as no hook at all', async () => {
    // An empty tool list is not grounding. `ungrounded_ref` already declines on
    // one, but `fabricated_outage` does not — so accepting it would let a
    // harness with no material assert `errorInfo: true` and start flagging
    // outage reports it cannot check.
    const hollow: HarnessDefinition<{ notes: string; failed: boolean }, string> = { ...synthesis, ground: () => ({ tools: [], results: '', errored: false }) }
    const r = world({ replies: [{ kind: 'fleet', text: 'The MCP server is unreachable.', toolNames: [], usage: null, contractDropped: false }] })
    const res = await runHarness(hollow, { notes: '', failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(res.findings).toEqual([])
  })

  it('a ground that throws is a missing record, not an escaped exception', async () => {
    const boom: HarnessDefinition<{ notes: string; failed: boolean }, string> = {
      ...synthesis,
      ground: (): never => {
        throw new TypeError("Cannot read properties of undefined (reading 'sources')")
      },
    }
    const r = world({ replies: ['See https://talaria.internal/tickets/9f9f9f9f.'] })
    const res = await runHarness(boom, { notes: HIT, failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(res.value).toBe('See https://talaria.internal/tickets/9f9f9f9f.')
    expect(res.findings).toEqual([])
  })

  it('fails open on grounding too large to scan', async () => {
    // guardrails.ts's own choice for an overflowing tool record, restated for a
    // declared one rather than re-decided.
    const huge: HarnessDefinition<{ notes: string; failed: boolean }, string> = {
      ...synthesis,
      ground: () => ({ tools: ['research_search'], results: 'x'.repeat(200_001), errored: false }),
    }
    const r = world({ replies: ['See https://talaria.internal/tickets/9f9f9f9f.'] })
    const res = await runHarness(huge, { notes: '', failed: false }, { caller: 'research:r1', deps: { ...r.deps, ...policed() } })
    expect(res.findings.map((f) => f.check)).not.toContain('ungrounded_ref')
  })
})

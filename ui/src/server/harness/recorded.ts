// THE RECORDED-TRANSCRIPT HARNESS: run any harness in the registry against
// written-down model replies, with no gateway, no fleet, no database and no
// clock.
//
// THIS IS AN EXTRACTION, NOT A NEW IDEA. `run.test.ts` has driven the runner
// this way since the runner existed — its `world()` helper builds exactly this
// `Partial<HarnessDeps>` — and that pattern IS the recorded-transcript harness
// the audit asks for. What it was not, was reachable: it lived inside one test
// file, so the fitness suite's tests, a harness author's tests, and (once the
// SDK ships `defineHarness`) an app author's tests each had to write a second
// copy. A second copy of a fake is worse than a second copy of real code,
// because the assertions it supports quietly become assertions about the fake.
// `run.test.ts` should import from here rather than keep its own; nothing else
// should ever grow a third.
//
// WHAT IS FAKE AND WHAT IS REAL, which is the whole design:
//
//   FAKE   the transport (scripted replies), model resolution, routing, the
//          capability record, the clock, and the two recorders. Every one of
//          them is an edge that would otherwise need a service.
//
//   REAL   the parser (`parseJson` runs, through the harness's own schema), the
//          guard rules (`gateSafeFindings` below runs the actual `RULES`
//          registry), and the persona resolver (`personaKeysFrom` over recorded
//          agent-config rows). Faking any of those would turn an assertion into
//          a restatement of the fake: a stubbed `guardText` that returns []
//          makes "the runner refuses to repair a reply carrying a credential"
//          pass on a runner that does no such thing.
//
// THE CAPABILITY DEFAULT IS UNKNOWN, deliberately. An absent fact is the state
// a fresh self-host is in, and `missingCapabilities` treats unknown as present
// (capability.ts's cardinal rule: only a fact that positively says "no" counts
// as missing). A helper that defaulted to "everything works" would hide every
// floor refusal, which is the behavior most worth testing.
import { RULES, type Finding, type GuardConfig } from '../guardrails'
import type { Capability, CapabilitySource } from './capability'
import type { ModelChainStep } from './model'
import { personaKeysFrom, type PersonaRow } from './persona'
import type { HarnessDeps, HarnessRunRow } from './run'
import type { Transport, TransportReply, TransportRequest } from './transport'

/** The guard config a recorded run reads unless a case says otherwise — the
 *  shipped default from guardrails.ts, restated here so a recorded run does not
 *  need a settings read. */
export const RECORDED_GUARD_CONFIG: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

/** A capability fact as a case writes it. A bare boolean is a `probe` fact — a
 *  deliberate measurement — because that is what most cases mean; the object
 *  form spells out the rest, and `learned` (what the gateway writes off a 400)
 *  behaves differently at the floor. */
export type RecordedFact = boolean | { value: boolean; source: CapabilitySource }

export interface RecordedWorld {
  /** Replies the transport hands back, IN ORDER, and THE LAST ONE REPEATS.
   *
   *  The repeat is load-bearing rather than convenient: a JSON harness that
   *  repairs sends a second call, and a case that supplied one bad reply means
   *  "this model always answers like this", not "this model answers badly once
   *  and then a fixture runs out". A `TransportReply` instead of a string is how
   *  a case says something about the CALL — tool names from a persona, a usage
   *  record, or `contractDropped` for the silent-strip case of audit 1.2. */
  replies?: Array<string | TransportReply>
  /** What the model chain resolves to. `null` means NOTHING routes, which is
   *  the state of an install that has never named an endpoint and is a real
   *  branch in the runner. Omitted means a pinned-looking `pl-main`. */
  model?: { model: string; step: ModelChainStep } | null
  /** Endpoints the model can land on — the capability keys are built from
   *  these. Defaults to one, or to none when the case supplies personas (a
   *  persona is not in the gateway catalog, so `routingFor` finds no endpoints
   *  for it, and that is precisely the condition capability keys used to be
   *  empty in). */
  endpoints?: string[]
  /** Capability facts, keyed by `endpoint:model` OR by a bare endpoint name as
   *  a shorthand meaning "every model this endpoint serves". Absent is UNKNOWN.
   *  See the header. */
  facts?: Record<string, Partial<Record<Capability, RecordedFact>>>
  /** Overrides on top of `RECORDED_GUARD_CONFIG`. */
  guard?: Partial<GuardConfig>
  /** Agent-version rows the REAL persona resolver reads. Supplying them is how
   *  a case tests tier resolution, which is the only question there. */
  personas?: PersonaRow[]
  /** The persona config lookup throws — the database is down mid-run. */
  personasThrow?: boolean
  /** How much the fake clock advances per reading, in ms. Every latency a
   *  recorded run reports is a multiple of this, so a test asserts on a number
   *  it chose rather than on how fast the machine happened to be. */
  tick?: number
}

/** Everything a recorded run lets a caller inspect afterwards. */
export interface RecordedRun {
  /** Every request the runner made, in order — the prompt it rendered, the
   *  temperature it asked for, whether it asked for JSON at the protocol level. */
  requests: TransportRequest[]
  /** The `harness_runs` rows the runner tried to write. This is the production
   *  ground truth the fitness page reads, so a test that asserts on it is
   *  asserting about the column an admin will actually see. */
  runs: HarnessRunRow[]
  /** The findings the runner tried to file. Grounded ones are already excluded
   *  by the runner, exactly as `recordFindings` excludes them. */
  findings: Finding[]
  /** Hand this to `runHarness` as `ctx.deps`. */
  deps: Partial<HarnessDeps>
}

/** THE GATE-SAFE RULES OVER PLAIN TEXT — what `guardText` does, minus its
 *  settings read, and running the REAL registry.
 *
 *  The runner calls `guardText` before it repairs, and that pass is what decides
 *  whether a bad reply is safe to hand back to a model. A stub that returned []
 *  would make every "the runner refuses to repair a reply carrying a
 *  credential" assertion vacuous, so this reproduces the real function's one
 *  subtlety: a `finding+redaction` hit is DROPPED here rather than returned, so
 *  the repair gate can never refuse a reply over a finding that does not exist
 *  in production. */
export function gateSafeFindings(text: string, input?: string, config: GuardConfig = RECORDED_GUARD_CONFIG): Finding[] {
  const out: Finding[] = []
  for (const rule of RULES) {
    if (!rule.gateSafe) continue
    if (!(config.checks[rule.id] ?? rule.defaultOn)) continue
    const hit = rule.run({
      answer: text,
      toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false },
      userMessage: '',
      ...(input ? { inputText: input } : {}),
      policedHosts: config.policedHosts,
    })
    if (!hit || hit.confidence < config.minConfidence) continue
    if (hit.grounded && rule.groundable === 'finding+redaction') continue
    out.push({
      check: rule.id,
      severity: rule.severity,
      confidence: hit.confidence,
      message: hit.message,
      snippet: hit.snippet,
      ...(hit.grounded && rule.groundable ? { grounded: true } : {}),
    })
  }
  return out
}

/** A transport that answers from a script and records what it was asked.
 *
 *  Exported on its own because it is the ONE edge a live-model caller wants to
 *  replace without replacing the rest: `runnerAsk(model, recordedTransport(...))`
 *  in fitness/probes.ts replays a probe, and `EvalDeps.harnessDeps.transport`
 *  replays a whole sweep, both against written-down replies. */
export function recordedTransport(replies: Array<string | TransportReply>): { transport: Transport; requests: TransportRequest[] } {
  const requests: TransportRequest[] = []
  const transport: Transport = async (req) => {
    requests.push(req)
    const reply = replies[Math.min(requests.length - 1, replies.length - 1)] ?? ''
    if (typeof reply !== 'string') return reply
    return { kind: 'gateway', text: reply, toolNames: [], usage: null, contractDropped: false }
  }
  return { transport, requests }
}

/** Build the injected edges for one recorded run. */
export function recordedRun(w: RecordedWorld = {}): RecordedRun {
  const runs: HarnessRunRow[] = []
  const findings: Finding[] = []
  const config: GuardConfig = { ...RECORDED_GUARD_CONFIG, ...w.guard }
  const { transport, requests } = recordedTransport(w.replies ?? ['{"ok":true}'])
  const endpoints = w.endpoints ?? (w.personas || w.personasThrow ? [] : ['spark'])
  const tick = w.tick ?? 7
  let clock = 0

  const factsFor = (key: string): Partial<Record<Capability, { value: boolean; source?: CapabilitySource }>> => {
    // Keys are `endpoint:model` (capability.ts). A full key wins; otherwise the
    // bare endpoint name applies to every model it serves.
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
    findings,
    deps: {
      resolveModel: async () => (w.model === undefined ? { model: 'pl-main', step: 'pin' } : w.model),
      routing: async (model) => ({ endpoints, upstreamModel: model }),
      // The REAL resolver over recorded rows: tier resolution is the whole
      // question here, so a fake that just handed back keys would assert nothing.
      personaKeys: async (model) => {
        if (w.personasThrow) throw new Error('connection terminated unexpectedly')
        return personaKeysFrom(model, w.personas ?? [])
      },
      // capability.ts's cardinal rule, reproduced exactly: only a fact that
      // positively says "no" counts as missing. Unknown is not missing.
      missingCapabilities: async (key, required) => {
        const facts = factsFor(key)
        return required.filter((cap) => facts[cap]?.value === false)
      },
      capabilities: async (key) => factsFor(key),
      transport,
      guardConfig: async () => config,
      guardText: async (text, input) => gateSafeFindings(text, input, config),
      recordFindings: async (hits) => {
        findings.push(...hits)
      },
      recordRun: async (row) => {
        runs.push(row)
      },
      now: () => (clock += tick),
    },
  }
}

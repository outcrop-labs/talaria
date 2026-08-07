// The Admin → Models → Fitness SURFACE: the matrix ("can I swap this model in,
// per role"), the run that fills it, the estimate that prices it, the archive
// that keeps last week's verdict, and the drill-down that makes a red cell
// trustworthy.
//
// WHY THIS IS A MODULE AND NOT THE ROUTE. `vitest.config.ts` excludes
// `src/routes/**` — in a file-based router a dot is a path separator, so
// `routes/api/foo.test.ts` is the handler for POST /api/foo/test, not a suite,
// and nothing under routes/ can be unit tested. The house rule that follows is
// written at the top of that config: route files parse the request, call ONE
// function in `src/server/*`, and serialize the result. Everything that is a
// DECISION lives here, where a test can reach it. `mergeFact` in particular
// decides whether a capability tag shows at all, which is to say it decides
// what an admin believes about a model, and it had no coverage at all while it
// lived in the route.
//
// THIS FILE ORCHESTRATES; IT SCORES NOTHING. Every band, rate and threshold
// comes from `fitness/score.ts`, every capability fact from
// `harness/capability.ts`, every production number from `fitness/observed.ts`.
// A second opinion about what "ready" means, written here so a panel could
// render faster, is exactly how the sentence in the UI and the arithmetic in
// the scorer come to disagree.
//
// WHAT LIVES HERE AND NOWHERE ELSE:
//   - the ARCHIVE. `fitness/*` computes a verdict and hands it back; nothing
//     persists one. The matrix is a page about models an admin tested LAST
//     WEEK, so the verdicts have to be kept.
//   - the RUN, across tiers. Tier 2 owns its own resumable status (the
//     `ReindexStatus` shape, in `app_settings`); tiers 1 and 3 finish in
//     seconds and have none. This file adds the one status row that says which
//     TIER is in flight, and follows the same shape rather than inventing a
//     second long-run mechanism.
//   - the ESTIMATE, composed. Each tier can size itself; only the caller knows
//     which tiers were asked for.
//
// EVERY EDGE IS INJECTED (`SurfaceDeps`), the same pattern `evals.ts` and
// `observed.ts` use: the gateway catalog, the settings rows, the three tier
// runners, the guard config and the clock. Defaults are the real ones, so the
// route passes nothing.
import { getSetting, setSetting } from '../audit'
import { getGuardConfig, type GuardConfig } from '../guardrails'
import { gatewayModels, routingFor, type GatewayModel, type ModelRouting } from '../llm-gateway'
import { listActivityHarnesses, type RegisteredHarness } from '../harness/registry'
import type { HarnessDefinition } from '../harness/define'
import {
  CAPABILITIES,
  capabilityKey,
  forgetCapabilities,
  getCapabilities,
  type Capability,
  type CapabilityFact,
  type CapabilityKey,
} from '../harness/capability'
import { estimateProbes, runProbes, type ProbeEstimate, type ProbeReport } from './probes'
import {
  evalSweepStatus,
  runEvalSweep,
  stopEvalSweep,
  type EvalCaseScore,
  type EvalSweep,
  type EvalSweepStatus,
  type HarnessScore,
} from './evals'
import {
  ADVERSARY_REQUIREMENT,
  estimateAdversarial,
  runAdversarial,
  type AdversarialBand,
  type AdversarialEstimate,
  type AdversarialReport,
} from './adversarial'
import {
  bindSlots,
  fitnessSlots,
  scoreFitness,
  slotKey,
  taskFloorFor,
  CONTRACT_READY,
  CONTRACT_UNFIT,
  REPAIR_WORKABLE,
  type FitnessBand,
  type FitnessReport,
  type FitnessSlot,
  type SlotBinding,
} from './score'
import {
  divergences,
  guardBaseline,
  observedHarnesses,
  observedModels,
  DEFAULT_WINDOW_DAYS,
  MIN_OBSERVED_RUNS,
  type Divergence,
  type ObservedHarness,
  type ObservedModel,
} from './observed'

// ── Storage ──────────────────────────────────────────────────────────────────

export type TierId = 'probes' | 'evals' | 'adversarial'
export const TIER_IDS: readonly TierId[] = ['probes', 'evals', 'adversarial']

export const isTierId = (v: string): v is TierId => (TIER_IDS as readonly string[]).includes(v)

/** THE MATRIX ROW, and the only thing the matrix read touches. Kept apart from
 *  the full report so drawing 30 models x 20 slots is ONE settings read rather
 *  than thirty multi-hundred-kilobyte ones. */
export interface FitnessIndexEntry {
  model: string
  /** When the run that produced these bands finished. The matrix prints it —
   *  a verdict with no date is a verdict an admin cannot judge the age of. */
  at: string
  tiers: TierId[]
  guarded: boolean
  /** slotKey → the cell. `reason` is `SlotVerdict.reasons[0].detail`, which
   *  score.ts sorts worst-band-first precisely so this is the right one. */
  cells: Record<string, { band: FitnessBand; reason: string | null }>
  /** Tier 3, which is a fact about the MODEL rather than about a slot, so it
   *  never colors a cell. See the note on `safety` in the payload below. */
  safety: { band: AdversarialBand; resistance: number | null } | null
  probesWrote: number
  costUsd: number | null
  calls: number
  /** The sweep did not finish (stopped, or interrupted by a deploy). Every
   *  unrun harness is already `untested` in the cells; this says the RUN was
   *  partial so the page can say so once, at the top, instead of the admin
   *  inferring it from a scatter of grey. */
  partial: boolean
}

export type FitnessIndex = Record<string, FitnessIndexEntry>

/** The full report for one model. One settings row per model, fetched only
 *  when an admin opens that model. */
export interface FitnessRecord {
  model: string
  at: string
  tiers: TierId[]
  report: FitnessReport
  harnesses: HarnessScore[]
  /** Drill-down cases. Bounded — see `DRILLDOWN_CAP`. */
  cases: EvalCaseScore[]
  droppedCases: number
  probes: ProbeReport | null
  adversarial: AdversarialReport | null
  sweep: { state: string; done: number; total: number; error: string | null; unfixtured: string[] }
}

/** THE LONG-RUN STATUS, in `app_settings`, deliberately the same shape and
 *  lifecycle as `ReindexStatus` (retrieval/migrate.ts). Talaria has one
 *  long-run mechanism; a second would be a second set of stuck-state bugs.
 *  Tier 2's own `EvalSweepStatus` carries the case counter and is merged in on
 *  read rather than copied here — two progress counters for one run is how
 *  they come to disagree. */
export interface FitnessRunStatus {
  state: 'idle' | 'running' | 'done' | 'error'
  model: string | null
  tiers: TierId[]
  phase: TierId | 'scoring' | null
  startedAt?: string
  finishedAt?: string
  error?: string
}

export type FitnessStatusView = FitnessRunStatus & { done: number; total: number; harness: string | null; sweepState: string }

export const INDEX_KEY = 'model_fitness_index'
export const STATUS_KEY = 'model_fitness_status'
export const BUDGET_KEY = 'model_fitness_budget'
export const recordKey = (model: string): string => `model_fitness_report:${model}`

export const IDLE: FitnessRunStatus = { state: 'idle', model: null, tiers: [], phase: null }

/** Drill-down cases kept per report. A sweep produces ~70; keeping every
 *  failing transcript for two dozen models turns `app_settings` into a
 *  transcript archive. Failed cases come first (they are the ones that carry a
 *  prompt and a reply at all); clean rows are cheap and are all kept. */
export const DRILLDOWN_CAP = 30
/** Models kept in the archive. A model nobody has tested in two dozen swaps is
 *  a model whose verdict is about weights that have since moved. */
export const KEEP_MODELS = 24

/** Per-case token counts MEASURED by the last sweep that ran each harness.
 *  This is what makes the tier-2 estimate an estimate rather than a guess: the
 *  fixtures are fixed, so the tokens one costs are close to the tokens it cost
 *  last time. A harness nobody has run yet contributes nothing and is COUNTED,
 *  so the UI can say the figure is a floor instead of quietly understating. */
export type TokenBudget = Record<string, { prompt: number; completion: number; at: string }>

// ── Injected edges ───────────────────────────────────────────────────────────

export interface SurfaceDeps {
  /** The gateway catalog: every callable id, bare and endpoint-qualified. */
  models: () => Promise<GatewayModel[]>
  /** Where a model id CAN land, with prices. Used for the estimate only. */
  routing: (model: string) => Promise<ModelRouting>
  capabilities: (key: CapabilityKey) => Promise<Partial<Record<Capability, CapabilityFact>>>
  forget: (key: CapabilityKey) => Promise<void>
  harnesses: () => Promise<RegisteredHarness[]>
  bindSlots: (harnesses: RegisteredHarness[]) => Promise<SlotBinding[]>
  readSetting: <T>(key: string, fallback: T) => Promise<T>
  writeSetting: (key: string, value: unknown) => Promise<void>
  estimateProbes: (model: string) => Promise<ProbeEstimate>
  runProbes: (model: string) => Promise<ProbeReport>
  estimateAdversarial: (opts: {
    adversaryModel?: string
    price?: (promptTokens: number, completionTokens: number) => Promise<number | null>
  }) => Promise<AdversarialEstimate>
  runAdversarial: (model: string, opts: { adversaryModel?: string }) => Promise<AdversarialReport>
  runEvalSweep: (model: string, opts: { restart: boolean; only?: string[] }) => Promise<EvalSweep>
  evalSweepStatus: () => Promise<EvalSweepStatus>
  stopEvalSweep: () => boolean
  guardConfig: () => Promise<GuardConfig>
  observedHarnesses: (opts?: { model?: string }) => Promise<ObservedHarness[]>
  observedModels: () => Promise<ObservedModel[]>
  /** ISO, injected so an archive test can pin the ordering the eviction sorts
   *  on rather than racing the wall clock. */
  nowIso: () => string
}

const REAL_DEPS: SurfaceDeps = {
  models: gatewayModels,
  routing: routingFor,
  capabilities: getCapabilities,
  forget: forgetCapabilities,
  harnesses: listActivityHarnesses,
  bindSlots,
  readSetting: getSetting,
  writeSetting: setSetting,
  estimateProbes: (model) => estimateProbes(model),
  runProbes: (model) => runProbes(model),
  estimateAdversarial: (opts) => estimateAdversarial(opts),
  runAdversarial: (model, opts) => runAdversarial(model, opts),
  runEvalSweep: (model, opts) => runEvalSweep(model, opts),
  evalSweepStatus,
  stopEvalSweep,
  guardConfig: getGuardConfig,
  observedHarnesses: (opts) => observedHarnesses(opts ?? {}),
  observedModels: () => observedModels(),
  nowIso: () => new Date().toISOString(),
}

const withDeps = (deps: Partial<SurfaceDeps> | undefined): SurfaceDeps => ({ ...REAL_DEPS, ...deps })

// ── Model rows and capability facts ──────────────────────────────────────────

export type CapabilityState = 'yes' | 'no' | 'unknown'

/** One capability as the UI shows it. `state` is three-valued and that is the
 *  whole point: KNOWN-TRUE, KNOWN-FALSE and NEVER-MEASURED are three different
 *  facts, and `missingCapabilities` only ever treats the middle one as a lack.
 *  A two-valued tag would turn every unprobed model on a fresh self-host into a
 *  wall of red. */
export interface CapabilityView {
  cap: Capability
  state: CapabilityState
  source: CapabilityFact['source'] | null
  detail: string | null
  score: number | null
  at: string | null
}

export interface ModelRow {
  id: string
  /** `endpoint/model` — one endpoint, one capability key, probeable. */
  qualified: boolean
  endpoints: string[]
  /** A bare id served by MORE THAN ONE endpoint. Capability is a property of
   *  the endpoint, so a pooled id's facts can disagree — and `runProbes`
   *  refuses to write under a pooled key for exactly that reason. The row is
   *  shown, its facts are shown where every member agrees, and the UI points at
   *  the endpoint-qualified ids for a run. */
  pooled: boolean
  capabilities: CapabilityView[]
}

/** The `endpoint:model` keys an id's facts live under, derived from the model
 *  catalog rather than by asking the router per model. `gatewayModels` builds
 *  qualified ids as `${endpoint}/${model}`, which is the same decomposition
 *  `routingFor` does — and doing it here costs one query for the whole page
 *  instead of one per model. */
export const keysFor = (row: { id: string; qualified: boolean; endpoints: string[] }): CapabilityKey[] => {
  const upstream = row.qualified ? row.id.slice(row.id.indexOf('/') + 1) : row.id
  return row.endpoints.map((ep) => capabilityKey(ep, upstream))
}

/** Display order for the capability tags, as a RANK rather than a list.
 *
 *  `Record<Capability, number>` is exhaustive by type, so a tenth capability
 *  fails the build here instead of quietly never rendering a tag. The list this
 *  replaced was the third hand-written copy of the same nine strings (the
 *  others: the probe registry and its census test, both now checked against
 *  `CAPABILITIES`). */
const CAPABILITY_RANK: Record<Capability, number> = {
  json: 0,
  'json-strict': 1,
  tools: 2,
  'tool-select': 3,
  search: 4,
  code: 5,
  'long-context': 6,
  vision: 7,
  'instruction-following': 8,
}

export const CAPABILITY_ORDER: Capability[] = [...CAPABILITIES].sort((a, b) => CAPABILITY_RANK[a] - CAPABILITY_RANK[b])

/** The sentence a pooled id gets when its endpoints disagree. Exported so the
 *  test asserts the string an admin actually reads rather than a paraphrase. */
export const POOLED_DISAGREEMENT =
  'The endpoints serving this model id disagree. Test the endpoint-qualified id instead.'

/** Merge one capability across every endpoint that could serve the id.
 *
 *  DISAGREEMENT IS UNKNOWN, NOT A VOTE. A bare id round-robins across its pool,
 *  so a call lands on one member; if the vendor API can hold JSON mode and the
 *  local llama.cpp build cannot, the honest answer for the pooled id is "it
 *  depends", and the only safe rendering of "it depends" is unmeasured. The
 *  alternative — crediting the better member — is the false `true` that
 *  `runProbes` refuses to write.
 *
 *  A MISSING MEMBER IS ALSO UNKNOWN, in BOTH directions: a pool where one
 *  endpoint says `true` and the other has never been measured is not a `yes`
 *  (the unmeasured one may well fail), and a pool where one says `false` and
 *  the other is unmeasured is not a `no` either — "unknown is not false" is the
 *  rule the whole capability model rests on, and downgrading an unmeasured
 *  member to a lack is what turns a fresh self-host into a wall of red. */
export function mergeFact(facts: ReadonlyArray<CapabilityFact | undefined>): Omit<CapabilityView, 'cap'> {
  const known = facts.filter((f): f is CapabilityFact => f !== undefined)
  if (known.length === 0 || known.length !== facts.length) {
    return { state: 'unknown', source: null, detail: null, score: null, at: null }
  }
  const first = known[0]!
  if (!known.every((f) => f.value === first.value)) {
    return { state: 'unknown', source: null, detail: POOLED_DISAGREEMENT, score: null, at: null }
  }
  return {
    state: first.value ? 'yes' : 'no',
    source: first.source,
    detail: first.detail ?? null,
    score: first.score ?? null,
    at: first.at,
  }
}

export async function modelRows(deps?: Partial<SurfaceDeps>): Promise<ModelRow[]> {
  const d = withDeps(deps)
  const models = await d.models()
  // One read per distinct endpoint:model, shared across the rows that mention
  // it — a bare id and its qualified sibling are the same key.
  const cache = new Map<CapabilityKey, Promise<Partial<Record<Capability, CapabilityFact>>>>()
  const factsFor = (key: CapabilityKey): Promise<Partial<Record<Capability, CapabilityFact>>> => {
    const hit = cache.get(key)
    if (hit) return hit
    const p = d.capabilities(key).catch(() => ({}) as Partial<Record<Capability, CapabilityFact>>)
    cache.set(key, p)
    return p
  }

  const rows: ModelRow[] = []
  for (const m of models) {
    const keys = keysFor(m)
    const perKey = await Promise.all(keys.map(factsFor))
    rows.push({
      id: m.id,
      qualified: m.qualified,
      endpoints: m.endpoints,
      pooled: !m.qualified && m.endpoints.length > 1,
      capabilities: CAPABILITY_ORDER.map((cap) => ({ cap, ...mergeFact(perKey.map((f) => f[cap])) })),
    })
  }
  return rows
}

/** Facts merged across every key the probe run wrote under, for scoring. A run
 *  against a pooled id writes nothing (the ambiguity rule in `runProbes`), so
 *  in practice this is one key. */
export function capabilitiesOf(row: ModelRow | undefined): Partial<Record<Capability, CapabilityFact>> {
  const out: Partial<Record<Capability, CapabilityFact>> = {}
  if (!row) return out
  for (const view of row.capabilities) {
    // Only a settled fact reaches the scorer. `unknown` must arrive as ABSENT,
    // because score.ts distinguishes "recorded false" (unfit) from "never
    // measured" (workable, run the probes) and a synthesized `false` would
    // collapse the two into the harsher one.
    if (view.state === 'unknown' || view.source === null || view.at === null) continue
    const fact: CapabilityFact = { value: view.state === 'yes', source: view.source, at: view.at }
    if (view.detail !== null) fact.detail = view.detail
    if (view.score !== null) fact.score = view.score
    out[view.cap] = fact
  }
  return out
}

// ── Pricing ──────────────────────────────────────────────────────────────────

export interface ModelPrice {
  in: number
  out: number
}

/** $/MTok for the DEAREST endpoint that could serve this model.
 *
 *  Dearest rather than average for the reason `fitness/probes.ts` gives for the
 *  identical derivation: an estimate the round-robin can exceed is not an
 *  estimate an admin can act on. That module keeps its copy private, so this is
 *  the second one — exporting it belongs to that file rather than here, and is
 *  the fix if a third ever appears. */
export async function priceOf(model: string, deps?: Partial<SurfaceDeps>): Promise<ModelPrice | null> {
  const d = withDeps(deps)
  const route = await d.routing(model).catch(() => null)
  if (!route || route.endpoints.length === 0) return null
  const priced = route.endpoints
    .map((ep) => {
      const over = ep.modelPrices?.[route.upstreamModel]
      const auto = ep.autoPrices?.[route.upstreamModel]
      const inTok = over?.in ?? auto?.in ?? ep.priceInPerMtok
      const outTok = over?.out ?? auto?.out ?? ep.priceOutPerMtok
      return typeof inTok === 'number' && typeof outTok === 'number' ? { in: inTok, out: outTok } : null
    })
    .filter((p): p is ModelPrice => p !== null)
  if (priced.length === 0) return null
  return priced.reduce((a, b) => (a.in + a.out >= b.in + b.out ? a : b))
}

export const usdOf = (price: ModelPrice | null, promptTokens: number, completionTokens: number): number | null =>
  price ? (promptTokens * price.in + completionTokens * price.out) / 1e6 : null

// ── The estimate ─────────────────────────────────────────────────────────────

export interface TierEstimate {
  tier: TierId
  calls: number
  promptTokens: number
  completionTokens: number
  usd: number | null
  /** Says what the tokens ARE, because they are not the same kind of number in
   *  every tier and an admin comparing them deserves to know which. */
  basis: 'fixture' | 'measured' | 'ceiling'
  note: string
}

export interface RunEstimate {
  model: string
  adversaryModel: string | null
  tiers: TierEstimate[]
  calls: number
  usd: number | null
  /** False when nothing prices this model. The call count is still exact, and
   *  it is the number that does not depend on a catalog being reachable. */
  priced: boolean
  /** Harnesses whose per-case tokens nothing has measured yet. Non-zero means
   *  the tier-2 dollar figure is a FLOOR, and the UI says so. */
  unmeasuredHarnesses: number
  fixtures: number
}

export interface Tier2Shape {
  harnesses: RegisteredHarness[]
  fixtures: number
  /** One repair turn per JSON fixture, worst case. `run.ts` sends a repair only
   *  when the contract fails, so this is a ceiling and never a surprise. */
  repairCeiling: number
}

export async function tier2Shape(only?: string[], deps?: Partial<SurfaceDeps>): Promise<Tier2Shape> {
  const d = withDeps(deps)
  const all = await d.harnesses()
  const harnesses = only?.length ? all.filter((h) => only.includes(h.id)) : all
  let fixtures = 0
  let repairCeiling = 0
  for (const h of harnesses) {
    fixtures += h.evalNames.length
    // Mirrors `metaOf` in evals.ts, which mirrors `maxRepairs` in run.ts: a
    // text harness never gets a repair turn, so budgeting one for it would
    // inflate every estimate on a registry that is thirteen-fourteenths text.
    const repairable = h.use(<I, O>(def: HarnessDefinition<I, O>) => def.output.kind === 'json' && (def.output.repair ?? 1) > 0)
    if (repairable) repairCeiling += h.evalNames.length
  }
  return { harnesses, fixtures, repairCeiling }
}

export interface EstimateRequest {
  model: string
  tiers: TierId[]
  adversaryModel: string | null
  only?: string[]
}

export async function estimateRun(req: EstimateRequest, deps?: Partial<SurfaceDeps>): Promise<RunEstimate> {
  const d = withDeps(deps)
  const { model, tiers, adversaryModel } = req
  const price = await priceOf(model, d)
  const rows: TierEstimate[] = []
  const shape = await tier2Shape(req.only, d)
  let unmeasured = 0

  if (tiers.includes('probes')) {
    // A probe that will skip costs nothing and `estimateProbes` already zeroes
    // its calls; this only sums what it hands back, so a skipped vision probe
    // subtracts from the total here without a second copy of the skip rule.
    const est = await d.estimateProbes(model).catch(() => null)
    rows.push({
      tier: 'probes',
      calls: est?.calls ?? 0,
      promptTokens: est?.promptTokens ?? 0,
      completionTokens: est?.completionTokens ?? 0,
      usd: est?.usd ?? null,
      basis: 'fixture',
      note: 'Fixed prompts, so this is exact — except the long-context probe, which is sized from the model’s own advertised window.',
    })
  }

  if (tiers.includes('evals')) {
    const budget = await d.readSetting<TokenBudget>(BUDGET_KEY, {})
    let promptTokens = 0
    let completionTokens = 0
    for (const h of shape.harnesses) {
      if (h.evalNames.length === 0) continue
      const b = budget[h.id]
      if (!b) {
        unmeasured++
        continue
      }
      promptTokens += b.prompt * h.evalNames.length
      completionTokens += b.completion * h.evalNames.length
    }
    rows.push({
      tier: 'evals',
      calls: shape.fixtures + shape.repairCeiling,
      promptTokens,
      completionTokens,
      usd: usdOf(price, promptTokens, completionTokens),
      basis: 'measured',
      note:
        unmeasured === 0
          ? 'Tokens are what these fixtures actually cost the last time each harness ran.'
          : `Tokens are what these fixtures cost the last time each harness ran. ${unmeasured} harness(es) have never run, so the figure is a floor.`,
    })
  }

  if (tiers.includes('adversarial')) {
    // The adversary is a different and usually dearer model, and the run pays
    // for both. Pricing every token at the dearer of the two keeps the number
    // a ceiling rather than a pleasant surprise in the wrong direction.
    const adversaryPrice = adversaryModel ? await priceOf(adversaryModel, d) : null
    const worst =
      price && adversaryPrice ? (price.in + price.out >= adversaryPrice.in + adversaryPrice.out ? price : adversaryPrice) : (adversaryPrice ?? price)
    const est = await d
      .estimateAdversarial({
        ...(adversaryModel ? { adversaryModel } : {}),
        price: async (p, c) => usdOf(worst, p, c),
      })
      .catch(() => null)
    rows.push({
      tier: 'adversarial',
      calls: (est?.calls ?? 0) + (est?.adversaryCalls ?? 0),
      promptTokens: est?.promptTokens ?? 0,
      completionTokens: est?.completionTokens ?? 0,
      usd: est?.costUsd ?? null,
      basis: 'ceiling',
      note: adversaryModel
        ? 'A ceiling: the escalation round only runs on seeds the model survives, and this assumes it survives all of them. Adversary calls are priced at the dearer of the two models.'
        : 'The seed corpus only. Naming an adversary adds an escalation round.',
    })
  }

  const usdRows = rows.filter((r) => r.usd !== null)
  return {
    model,
    adversaryModel,
    tiers: rows,
    calls: rows.reduce((n, r) => n + r.calls, 0),
    // Null unless EVERY requested tier priced. A partial total under a dollar
    // sign is a number nobody can reconcile with the invoice.
    usd: usdRows.length === rows.length && rows.length > 0 ? usdRows.reduce((n, r) => n + (r.usd ?? 0), 0) : null,
    priced: price !== null,
    unmeasuredHarnesses: unmeasured,
    fixtures: shape.fixtures,
  }
}

// ── The archive ──────────────────────────────────────────────────────────────

/** THE CAP APPLIES TO TRANSCRIPTS, NOT TO CASES.
 *
 *  `EvalCaseScore` carries a prompt and a reply only for cases that failed
 *  something, and those are the only rows with any size to them. Every clean
 *  case is a handful of numbers and is kept whole — dropping them would leave
 *  the panel unable to say how many fixtures actually passed, which is a worse
 *  trade than a bounded settings row. `dropped` therefore counts transcripts an
 *  admin cannot see, and nothing else. */
export function drilldown(cases: readonly EvalCaseScore[], cap: number = DRILLDOWN_CAP): { kept: EvalCaseScore[]; dropped: number } {
  const heavy = (c: EvalCaseScore): boolean => c.prompt !== null || c.raw !== null
  const withTranscript = cases.filter(heavy)
  const rest = cases.filter((c) => !heavy(c))
  return { kept: [...withTranscript.slice(0, cap), ...rest], dropped: Math.max(0, withTranscript.length - cap) }
}

/** Newest `keep` models survive; the rest are named so their report rows can go
 *  with them — an orphaned report row is a settings row nothing will ever read
 *  again.
 *
 *  Pure, and returns a NEW index rather than mutating: the caller writes the
 *  index and the report deletions in one step, and a half-applied eviction that
 *  had already mutated the caller's object is a matrix listing models whose
 *  reports are gone. */
export function evictArchive(index: FitnessIndex, keep: number = KEEP_MODELS): { index: FitnessIndex; evicted: string[] } {
  const ordered = Object.values(index).sort((a, b) => b.at.localeCompare(a.at))
  const evicted = ordered.slice(keep).map((e) => e.model)
  const kept: FitnessIndex = { ...index }
  for (const model of evicted) delete kept[model]
  return { index: kept, evicted }
}

export interface IndexEntryParts {
  model: string
  at: string
  ran: TierId[]
  requested: TierId[]
  sweep: EvalSweep
  report: FitnessReport
  probes: ProbeReport | null
  adversarial: AdversarialReport | null
}

/** The matrix row, assembled from what the run produced. */
export function indexEntryOf(parts: IndexEntryParts): FitnessIndexEntry {
  const { sweep, adversarial, probes } = parts
  const cells: FitnessIndexEntry['cells'] = {}
  for (const slot of parts.report.slots) {
    cells[slotKey(slot.slot)] = { band: slot.band, reason: slot.reasons[0]?.detail ?? null }
  }
  // Null unless EVERY component priced. A partial total under a dollar sign
  // is a number nobody can reconcile with the invoice, which is worse than
  // no number — the same reason `EvalDeps.price` defaults to null.
  const costs: Array<number | null> = [...sweep.harnesses.map((h) => h.costUsd), ...(adversarial ? [adversarial.costUsd] : [])]
  const costUsd = costs.length === 0 || costs.some((p) => p === null) ? null : costs.reduce<number>((n, p) => n + (p ?? 0), 0)
  return {
    model: parts.model,
    at: parts.at,
    tiers: parts.ran,
    guarded: sweep.guarded,
    cells,
    safety: adversarial ? { band: adversarial.band, resistance: adversarial.resistance } : null,
    probesWrote: probes?.wrote ?? 0,
    costUsd,
    calls: sweep.done + (adversarial?.cases.length ?? 0) + (probes?.results.length ?? 0),
    // Partial in either of the two ways a run can be: tier 2 stopped
    // mid-sweep, or a tier the admin asked for never produced a result.
    partial: sweep.state === 'stopped' || (sweep.total > 0 && sweep.done < sweep.total) || parts.ran.length < parts.requested.length,
  }
}

/** An empty sweep, for a run that skipped tier 2.
 *
 *  Every bound harness then lands on `not-swept` → `untested`, which is the
 *  correct reading: probes alone can turn a cell RED (a required capability
 *  came back false) but can never turn one green. A page that showed probe
 *  passes as Ready would be claiming the harnesses were exercised. */
export const emptySweep = (model: string, unfixtured: string[], guarded: boolean, at: string): EvalSweep => ({
  model,
  state: 'idle',
  startedAt: at,
  finishedAt: at,
  done: 0,
  total: 0,
  error: null,
  harnesses: [],
  cases: [],
  unfixtured,
  guarded,
})

async function recordBudget(harnesses: readonly HarnessScore[], d: SurfaceDeps): Promise<void> {
  if (harnesses.length === 0) return
  const at = d.nowIso()
  const budget = await d.readSetting<TokenBudget>(BUDGET_KEY, {})
  for (const h of harnesses) {
    if (h.cases === 0) continue
    budget[h.id] = { prompt: Math.round(h.promptTokens / h.cases), completion: Math.round(h.completionTokens / h.cases), at }
  }
  await d.writeSetting(BUDGET_KEY, budget).catch(() => {})
}

// ── The run ──────────────────────────────────────────────────────────────────

/** In-process, exactly like `reindexRunning` in retrieval/migrate.ts and
 *  `sweeping` in evals.ts: one node runs the tiers and the Stop button reaches
 *  the same node. */
let running = false
/** Stop, honored BETWEEN tiers as well as inside tier 2. `stopEvalSweep` only
 *  reaches the sweep; a run stopped during the probes would otherwise go on to
 *  buy the whole tier-2 sweep the admin just asked it not to. */
let stopRequested = false

/** Claim the run slot SYNCHRONOUSLY. Two simultaneous Start presses both clear
 *  an `if (running)` written above an `await` — the check and the claim have to
 *  be one step, and the caller owns releasing it (the `finally` in
 *  `runFitness`). */
function claimRun(): boolean {
  if (running) return false
  running = true
  stopRequested = false
  return true
}

export interface StartOptions {
  model: string
  tiers: TierId[]
  adversaryModel: string | null
  only?: string[]
  restart: boolean
}

/** Run the requested tiers against one candidate, score, archive.
 *
 *  DETACHED, like `reindexAll`: the tiers are minutes of model calls and the
 *  admin watches the status row. Every tier is individually `catch`ed — a
 *  probe suite that cannot reach the gateway must not void a tier-2 sweep the
 *  org already paid for.
 *
 *  THE CALLER HAS ALREADY CLAIMED the run slot (`claimRun`); this function
 *  releases it. Claiming here instead would put an await between the check and
 *  the claim in the route, which is a second concurrent run. */
export async function runFitness(opts: StartOptions, deps?: Partial<SurfaceDeps>): Promise<void> {
  const d = withDeps(deps)
  const { model, tiers, adversaryModel } = opts
  const startedAt = d.nowIso()
  const writeStatus = (s: FitnessRunStatus): Promise<void> => d.writeSetting(STATUS_KEY, s)
  const setPhase = (phase: FitnessRunStatus['phase']): Promise<void> =>
    writeStatus({ state: 'running', model, tiers, phase, startedAt }).catch(() => {})

  // The tiers that actually PRODUCED SOMETHING, which is not the tiers that
  // were asked for once Stop — or a dead gateway — is in play. The archived
  // record is stamped with this one: a record claiming a tier that never
  // happened is the same lie as a green cell nobody filled, and a tier whose
  // runner threw did not happen even though it was attempted.
  const ran: TierId[] = []

  try {
    await setPhase(tiers[0] ?? 'scoring')

    let probes: ProbeReport | null = null
    if (tiers.includes('probes') && !stopRequested) {
      await setPhase('probes')
      probes = await d.runProbes(model).catch(() => null)
      if (probes) ran.push('probes')
    }

    const harnesses = await d.harnesses()
    let sweep: EvalSweep | null = null
    if (tiers.includes('evals') && !stopRequested) {
      await setPhase('evals')
      sweep = await d.runEvalSweep(model, { restart: opts.restart, ...(opts.only?.length ? { only: opts.only } : {}) }).catch(() => null)
      if (sweep) {
        await recordBudget(sweep.harnesses, d)
        ran.push('evals')
      }
    }

    let adversarial: AdversarialReport | null = null
    if (tiers.includes('adversarial') && !stopRequested) {
      await setPhase('adversarial')
      adversarial = await d.runAdversarial(model, adversaryModel ? { adversaryModel } : {}).catch(() => null)
      if (adversarial) ran.push('adversarial')
    }

    // NOTHING RAN: archive nothing. Overwriting a real verdict from last week
    // with an empty one — every cell reset to Untested — would make Stop
    // destructive, and Stop is the button an admin reaches for when a run is
    // costing more than they expected. The same rule saves the archive when
    // every tier threw.
    if (ran.length === 0) {
      await writeStatus({ state: 'done', model, tiers, phase: null, startedAt, finishedAt: d.nowIso() })
      return
    }

    await setPhase('scoring')
    const unfixtured = harnesses.filter((h) => h.evalNames.length === 0).map((h) => h.id)
    // With the guard off every guard rate is zero, and zero-because-off must
    // not read as zero-because-clean — score.ts caps such a run at `workable`
    // and says why. A run that skipped tier 2 has no sweep to ask, so the
    // install's own mode is read rather than a `false` stood in for it, which
    // would report the guard as off on an install where it is on.
    const guarded = await d
      .guardConfig()
      .then((c) => c.mode !== 'off')
      .catch(() => false)
    const effective = sweep ?? emptySweep(model, unfixtured, guarded, startedAt)
    const observed = await d.observedHarnesses().catch((): ObservedHarness[] => [])
    const rows = await modelRows(d).catch((): ModelRow[] => [])
    const report = scoreFitness(
      {
        sweep: effective,
        harnesses,
        capabilities: capabilitiesOf(rows.find((r) => r.id === model)),
        guardBaseline: guardBaseline(observed),
      },
      await d.bindSlots(harnesses),
    )

    const at = d.nowIso()
    const { kept, dropped } = drilldown(effective.cases)
    const record: FitnessRecord = {
      model,
      at,
      tiers: ran,
      report,
      harnesses: effective.harnesses,
      cases: kept,
      droppedCases: dropped,
      probes,
      // A dozen provocations, of which only the ones the model FELL for carry
      // a transcript. Kept whole: this is the tier whose drill-down an admin is
      // most likely to need in order to justify a decision.
      adversarial,
      sweep: {
        state: effective.state,
        done: effective.done,
        total: effective.total,
        error: effective.error,
        unfixtured: effective.unfixtured,
      },
    }
    await d.writeSetting(recordKey(model), record)

    const entry = indexEntryOf({ model, at, ran, requested: tiers, sweep: effective, report, probes, adversarial })
    const stored = await d.readSetting<FitnessIndex>(INDEX_KEY, {})
    const { index, evicted } = evictArchive({ ...stored, [model]: entry })
    for (const stale of evicted) await d.writeSetting(recordKey(stale), null).catch(() => {})
    await d.writeSetting(INDEX_KEY, index)

    await writeStatus({ state: 'done', model, tiers, phase: null, startedAt, finishedAt: d.nowIso() })
  } catch (e) {
    await writeStatus({
      state: 'error',
      model,
      tiers,
      phase: null,
      startedAt,
      finishedAt: d.nowIso(),
      error: (e as Error).message,
    }).catch(() => {})
  } finally {
    running = false
  }
}

// ── Payloads ─────────────────────────────────────────────────────────────────

/** The band thresholds, sent rather than restated in a Svelte file. A cell
 *  tooltip prints "contract 91%, Ready needs 95%", and a second copy of 0.95 in
 *  the client is how that sentence and score.ts come to disagree. */
export const THRESHOLDS = {
  contractReady: CONTRACT_READY,
  contractUnfit: CONTRACT_UNFIT,
  repairWorkable: REPAIR_WORKABLE,
  observedWindowDays: DEFAULT_WINDOW_DAYS,
  minObservedRuns: MIN_OBSERVED_RUNS,
}

export interface SlotView extends FitnessSlot {
  key: string
  taskFloor: number
}

export const slotViews = (): SlotView[] => fitnessSlots().map((s) => ({ ...s, key: slotKey(s), taskFloor: taskFloorFor(s) }))

export async function fitnessStatus(deps?: Partial<SurfaceDeps>): Promise<FitnessStatusView> {
  const d = withDeps(deps)
  const [status, sweep] = await Promise.all([d.readSetting<FitnessRunStatus>(STATUS_KEY, IDLE), d.evalSweepStatus().catch(() => null)])
  // The case counter belongs to tier 2 and is READ from it, never mirrored.
  const live = status.state === 'running' && sweep?.model === status.model
  return {
    ...status,
    done: live ? sweep.done : 0,
    total: live ? sweep.total : 0,
    harness: live ? sweep.harness : null,
    sweepState: sweep?.state ?? 'idle',
  }
}

export interface MatrixView {
  slots: SlotView[]
  models: ModelRow[]
  index: FitnessIndex
  status: FitnessStatusView
  thresholds: typeof THRESHOLDS
  registry: { harnesses: number; fixtures: number; unfixtured: string[] }
}

export interface CapabilitiesView {
  models: ModelRow[]
  index: FitnessIndex
}

export interface EstimateView {
  estimate: RunEstimate
  adversaryRequirement: typeof ADVERSARY_REQUIREMENT
}

export interface DetailView {
  model: string
  record: FitnessRecord | null
  observed: ObservedHarness[]
  observedModel: ObservedModel | null
  divergences: Divergence[]
  thresholds: typeof THRESHOLDS
}

/** What the GET verb was asked for, already parsed off the query string by the
 *  route. Everything below this line is a decision; the parsing above it is
 *  HTTP plumbing and stays in the route. */
export interface FitnessQuery {
  view: string
  model: string | null
  tiers: string | null
  adversary: string | null
  only: string | null
}

export type FitnessGetResult =
  | { ok: true; body: MatrixView | CapabilitiesView | EstimateView | DetailView }
  /** A 400 with a sentence. The route maps it; nothing here knows about codes. */
  | { ok: false; error: string }

export async function readFitness(query: FitnessQuery, deps?: Partial<SurfaceDeps>): Promise<FitnessGetResult> {
  const d = withDeps(deps)
  const { model } = query

  if (query.view === 'capabilities') {
    // The index rides along: the panels that PICK a model need the band its
    // last run gave the slot they are assigning, and a second round trip for
    // two dozen small cell maps would be a request per panel per open.
    return { ok: true, body: { models: await modelRows(d), index: await d.readSetting<FitnessIndex>(INDEX_KEY, {}) } }
  }

  if (query.view === 'estimate') {
    if (!model) return { ok: false, error: 'model is required' }
    const tiers = (query.tiers ?? 'probes,evals').split(',').filter(isTierId)
    if (tiers.length === 0) return { ok: false, error: 'pick at least one tier' }
    const only = query.only?.split(',').filter(Boolean)
    return {
      ok: true,
      body: {
        estimate: await estimateRun({ model, tiers, adversaryModel: query.adversary, ...(only ? { only } : {}) }, d),
        adversaryRequirement: ADVERSARY_REQUIREMENT,
      },
    }
  }

  if (query.view === 'detail') {
    if (!model) return { ok: false, error: 'model is required' }
    const record = await d.readSetting<FitnessRecord | null>(recordKey(model), null)
    // Production telemetry is ADVISORY and is fetched even with no bench
    // record: "this model is running in production and has never been
    // tested" is one of the more useful things this page can say.
    const [observed, models] = await Promise.all([
      d.observedHarnesses({ model }).catch((): ObservedHarness[] => []),
      d.observedModels().catch((): ObservedModel[] => []),
    ])
    return {
      ok: true,
      body: {
        model,
        record,
        observed,
        observedModel: models.find((m) => m.model === model) ?? null,
        divergences: record ? divergences(model, record.harnesses, observed) : [],
        thresholds: THRESHOLDS,
      },
    }
  }

  const [models, index, status, shape] = await Promise.all([
    modelRows(d),
    d.readSetting<FitnessIndex>(INDEX_KEY, {}),
    fitnessStatus(d),
    tier2Shape(undefined, d),
  ])
  return {
    ok: true,
    body: {
      slots: slotViews(),
      models,
      index,
      status,
      thresholds: THRESHOLDS,
      registry: {
        harnesses: shape.harnesses.length,
        fixtures: shape.fixtures,
        unfixtured: shape.harnesses.filter((h) => h.evalNames.length === 0).map((h) => h.id),
      },
    },
  }
}

// ── The write verbs ──────────────────────────────────────────────────────────

export type StartResult =
  /** 409. The second press of Start means "show me the run", not "start a
   *  second one" — the same call the tier-2 sweep makes for itself. */
  | { ok: false; reason: 'busy'; status: FitnessStatusView }
  /** 400, with the sentence the admin reads. */
  | { ok: false; reason: 'rejected'; error: string }
  | { ok: true; status: FitnessStatusView }

/** Validate, claim the run slot, and detach the run. */
export async function startFitnessRun(
  req: { model: string; tiers: TierId[]; adversaryModel: string | null; only?: string[]; restart: boolean },
  deps?: Partial<SurfaceDeps>,
): Promise<StartResult> {
  const d = withDeps(deps)
  // A fast 409 for the common case, before the catalog reads. Not the door —
  // `claimRun` below is, because only it is synchronous with the claim.
  if (running) return { ok: false, reason: 'busy', status: await fitnessStatus(d) }

  const rows = await modelRows(d)
  if (!rows.some((r) => r.id === req.model)) return { ok: false, reason: 'rejected', error: 'that model is not on the gateway' }
  if (req.adversaryModel) {
    if (!rows.some((r) => r.id === req.adversaryModel)) {
      return { ok: false, reason: 'rejected', error: 'that adversary model is not on the gateway' }
    }
    if (req.adversaryModel === req.model) {
      // A model grading its own resistance is the who-judges-the-judge regress
      // with the stakes turned up (adversarial.ts says so in prose; this is the
      // door).
      return { ok: false, reason: 'rejected', error: 'the adversary must be a different model than the candidate' }
    }
  }

  const opts: StartOptions = {
    model: req.model,
    tiers: req.tiers,
    adversaryModel: req.adversaryModel,
    restart: req.restart,
    ...(req.only?.length ? { only: req.only } : {}),
  }
  // Claimed here, with no await between the check and the claim.
  if (!claimRun()) return { ok: false, reason: 'busy', status: await fitnessStatus(d) }
  await d.writeSetting(STATUS_KEY, {
    state: 'running',
    model: opts.model,
    tiers: opts.tiers,
    phase: opts.tiers[0] ?? null,
    startedAt: d.nowIso(),
  } satisfies FitnessRunStatus)
  void runFitness(opts, deps).catch(() => {})
  return { ok: true, status: await fitnessStatus(d) }
}

export async function stopFitnessRun(deps?: Partial<SurfaceDeps>): Promise<{ stopped: boolean; status: FitnessStatusView }> {
  const d = withDeps(deps)
  // Two stops, because there are two things running: the tier-2 sweep, which
  // honors it at a case boundary and stays RESUMABLE, and the tier loop, which
  // honors it by not buying the tiers that have not started.
  stopRequested = true
  const stopped = d.stopEvalSweep() || running
  return { stopped, status: await fitnessStatus(d) }
}

export type ForgetResult = { ok: true; keys: CapabilityKey[]; models: ModelRow[] } | { ok: false; error: string }

/** Audit 1.2's release valve, per endpoint:model rather than per id: a model id
 *  re-pointed at different weights has facts about something else, and the
 *  gateway's learned-parameter ratchet has no other way out. */
export async function forgetModel(model: string, deps?: Partial<SurfaceDeps>): Promise<ForgetResult> {
  const d = withDeps(deps)
  const rows = await modelRows(d)
  const row = rows.find((r) => r.id === model)
  if (!row) return { ok: false, error: 'that model is not on the gateway' }
  const keys = keysFor(row)
  for (const key of keys) await d.forget(key)
  return { ok: true, keys, models: await modelRows(d) }
}

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
import { reachFor, supplierFor, PROVIDERS_KEY, type CapabilityProviders, type PlatformSupply, type Reach } from '../capability-reach'
import { platformSupply } from '../capability-platform'
import { capabilityKeysFor } from '../harness/run'
import { canonicalModelId, gatewayModels, routingFor, type GatewayModel, type ModelRouting } from '../llm-gateway'
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
import { clearTranscripts, pruneTranscripts, readTranscripts, recordTranscript, transcriptRuns, type Transcript } from './transcripts'
import { summarize, type HealthInput, type HealthSummary } from './health'
import { listMcpServers, type McpServer } from '../mcp-registry'
import {
  clearEvalStatus,
  evalSweepStatuses,
  inFlightFor,
  runEvalSweep,
  stopEvalSweep,
  type EvalCaseScore,
  type InFlightCase,
  type EvalSweep,
  type EvalSweepStatus,
  type HarnessScore,
  type SweepConcurrency,
} from './evals'
import {
  ADVERSARY_REQUIREMENT,
  SEEDS,
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
import { harnessSummary, valueView, type HarnessSummary, type ValueView } from './value'

// ── Storage ──────────────────────────────────────────────────────────────────

export type TierId = 'probes' | 'evals' | 'adversarial'
export const TIER_IDS: readonly TierId[] = ['probes', 'evals', 'adversarial']

export const isTierId = (v: string): v is TierId => (TIER_IDS as readonly string[]).includes(v)

/** THE MATRIX ROW, and the only thing the matrix read touches. Kept apart from
 *  the full report so drawing 30 models x 20 slots is ONE settings read rather
 *  than thirty multi-hundred-kilobyte ones. */
/** The speed half of a run, computed once where the sweep is scored. */
export interface SpeedReading {
  /** THE HEADLINE: output tokens per second, median over the cases this pass
   *  measured.
   *
   *  WHY NOT TIME-PER-CASE, which this used to lead with. A fixture that asks
   *  for a SKILL.md takes longer than one that asks for a chat title on any
   *  model, so a per-case latency is mostly a fact about which fixtures ran —
   *  and it moves whenever the corpus does. Tokens per second divides that out:
   *  it is the rate the model generates at, comparable between two models that
   *  ran different fixtures and between two runs of a corpus that changed.
   *
   *  Null when nothing measured both a duration and a completion, which is the
   *  honest answer for a sweep of contract failures. */
  tokensPerSecond: number | null
  /** Median per-case latency, ms — the runner's own measure, so it is the same
   *  number `harness_runs` records and observed-vs-tested compares against.
   *  Kept beside the rate because "how fast does it generate" and "how long do I
   *  wait for a fixture" are both real questions. */
  p50: number
  p95: number
  /** Wall clock of the whole sweep, from the first case starting to the last
   *  one finishing. NOT the sum of the latencies: under concurrency the run is
   *  shorter than its parts, and with retries a case costs more than it says. */
  elapsedMs: number
  /** Fixtures per minute — the figure that answers "how long will testing the
   *  next candidate take me". */
  perMinute: number
  /** Cases in flight while this was measured. See `FitnessIndexEntry.speed`. */
  concurrency: number
  /** HOW MANY CASES THIS READING IS OVER. A supplemental pass of seven fixtures
   *  gives a real but small sample, and a median over seven is a different claim
   *  from a median over two hundred and forty — the panel says which. */
  sample: number
}

/** Speed, from the cases a sweep recorded. Pure, so the matrix column and the
 *  report card cannot disagree about the same run. */
export function speedOf(cases: readonly EvalCaseScore[], concurrency: number): SpeedReading | null {
  // OVER THE CASES THIS PASS RAN, never over the whole ledger — see
  // `EvalSweep.measured`. A supplemental pass that ran seven fixtures must not
  // report a latency computed from two hundred and forty inherited ones measured
  // last week at a different width.
  //
  // MEASURED CASES ONLY. A skip never called the model and a case the provider
  // never answered has no latency to speak of; averaging their zeros in would
  // make a badly-served model look fast.
  const scored = cases.filter((c) => c.skipped === null && c.latencyMs > 0)
  if (scored.length === 0) return null
  const sorted = scored.map((c) => c.latencyMs).sort((a, b) => a - b)
  const at = (q: number): number => sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1))] ?? 0
  // PER CASE, THEN THE MEDIAN — not total tokens over total time. Under
  // concurrency the wall clock overlaps, so an aggregate would report a rate no
  // single request ever achieved; and one long generation would dominate the
  // sum. Each case's own rate is a fact about that request, and the median of
  // them is a fact about the model.
  const rates = scored
    .filter((c) => c.completionTokens > 0 && c.latencyMs > 0)
    .map((c) => c.completionTokens / (c.latencyMs / 1000))
    .sort((a, b) => a - b)
  const tokensPerSecond = rates.length === 0 ? null : Math.round((rates[Math.floor((rates.length - 1) / 2)] ?? 0) * 10) / 10
  const starts = cases.map((c) => Date.parse(c.startedAt)).filter((n) => Number.isFinite(n) && n > 0)
  const ends = cases.map((c) => Date.parse(c.startedAt) + c.wallMs).filter((n) => Number.isFinite(n) && n > 0)
  const elapsedMs = starts.length && ends.length ? Math.max(0, Math.max(...ends) - Math.min(...starts)) : 0
  return {
    tokensPerSecond,
    p50: at(0.5),
    p95: at(0.95),
    elapsedMs,
    perMinute: elapsedMs > 0 ? Math.round((cases.length / elapsedMs) * 60_000 * 10) / 10 : 0,
    concurrency,
    sample: scored.length,
  }
}

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
  /** HOW FAST THIS MODEL IS IN THIS INSTALL, measured over the same 247
   *  fixtures every candidate runs — which is what makes the number comparable
   *  down a column at all. Null when tier 2 did not run.
   *
   *  IT CARRIES THE WIDTH IT WAS MEASURED AT, and that is not decoration: at
   *  four in flight a per-case latency includes queueing at the provider, so a
   *  p50 from a 4-wide sweep and a p50 from a sequential one are two different
   *  measurements wearing one name. The column says so rather than letting an
   *  admin compare them silently. */
  speed: SpeedReading | null
  costUsd: number | null
  calls: number
  /** The sweep did not finish (stopped, or interrupted by a deploy). Every
   *  unrun harness is already `untested` in the cells; this says the RUN was
   *  partial so the page can say so once, at the top, instead of the admin
   *  inferring it from a scatter of grey. */
  partial: boolean
  /** PER HARNESS, the small half of the report — what the value view needs to
   *  weigh a verdict against how much of a real day that harness is, without
   *  reading two dozen multi-hundred-kilobyte records to draw one table.
   *
   *  `band` is `harnessBands`' worst-across-slots collapse, and covers the
   *  UNBOUND harnesses the cells cannot speak for. `prompt`/`completion` are
   *  per-case tokens THIS model spent, which is the only basis on which a terse
   *  model and a chatty one price differently.
   *
   *  OPTIONAL, and stays that way: entries archived before this field existed
   *  are still valid rows. `value.ts` backfills one from the full report on
   *  read, and degrades to the cells and the shared token budget if even that
   *  is gone — never a demand to re-test a model an admin already paid for. */
  harnesses?: Record<string, HarnessSummary>
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
  /** WHEN EACH TIER WAS LAST MEASURED. A record is merged across runs — a tier
   *  that did not run keeps its previous result — so `at` alone would put
   *  today's date over a month-old probe result. */
  tierAt: Partial<Record<TierId, string>>
  sweep: {
    state: string
    done: number
    total: number
    error: string | null
    unfixtured: string[]
    /** HOW WIDE IT RAN, archived with the run. Without it a p50 from a 4-wide
     *  sweep and a p50 from a sequential one are the same field holding two
     *  different measurements, and the page would compare them silently. */
    concurrency: SweepConcurrency
  }
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

/** Every run the page draws, newest first, plus what the panel needs to know
 *  about whether it may start another. */
export interface FitnessRunsView {
  runs: FitnessStatusView[]
  /** How many may run at once, sent rather than restated in a Svelte file. */
  max: number
  /** True when a further Start would be refused. */
  full: boolean
}

export const INDEX_KEY = 'model_fitness_index'
/** THE RUN STATUS STORE, keyed by candidate — see the note on `RUNS_KEY` in
 *  evals.ts for why this is a new key rather than a reshape of the old one: the
 *  status row of a run already in flight is written by code already loaded, and
 *  a shape changed under it loses the row. `STATUS_KEY` is still READ, folded
 *  in under its own model, and nothing writes it again. */
export const RUNS_KEY = 'model_fitness_runs'

/** MODELS ASKED TO STOP, IN THE DATABASE.
 *
 *  THE STOP BUTTON DID NOT WORK, and this is why. The request was a boolean on
 *  an in-process map, so it could only ever reach a run whose closure lived in
 *  the module instance the request happened to hit. In dev that is one HMR
 *  reload away from being a different instance — a sweep started before a
 *  server-side edit had a Stop button that returned `stopped: false` while the
 *  run carried on for another twenty minutes. The same hole exists across a
 *  restart, and across processes on any deployment with more than one.
 *
 *  A settings row is the one thing every instance can see. The in-process flag
 *  stays as the fast path (a run in THIS instance stops on the next case
 *  without a read); this is what makes the request survive everything else. */
export const STOP_KEY = 'model_fitness_stop'
export const STATUS_KEY = 'model_fitness_status'
export const BUDGET_KEY = 'model_fitness_budget'
export const recordKey = (model: string): string => `model_fitness_report:${model}`

/** AN ARCHIVED RECORD WAS WRITTEN BY AN OLDER VERSION OF THIS FILE, and it is
 *  read by the current one. Every field added to `FitnessRecord` after a run was
 *  archived is missing from that run, and the type says otherwise — so a panel
 *  that reads `record.sweep.concurrency.ended` throws on a report from last
 *  week and takes the whole route down with it.
 *
 *  THAT IS EXACTLY WHAT HAPPENED. `sweep.concurrency` was added with the
 *  parallel sweep, every previously archived report lacked it, and the Models
 *  page stopped rendering for anybody who had ever tested a model.
 *
 *  So the archive is upgraded ON READ, in one place, rather than every consumer
 *  guarding every field. An old run genuinely ran one case at a time, so `1` is
 *  not a placeholder here — it is the truth about that run. */
/** `low` arrived with the two-way valve. Before it, the valve only ever closed,
 *  so the width a run ENDED at was also the narrowest it ever reached — which
 *  makes `ended` the correct backfill rather than a guess. */
function upgradeConcurrency(c: SweepConcurrency | undefined): SweepConcurrency {
  if (!c) return { requested: 1, ended: 1, low: 1, narrowedBecause: null }
  return { ...c, low: c.low ?? c.ended }
}

function upgradeRecord(record: FitnessRecord | null): FitnessRecord | null {
  if (!record) return null
  return {
    ...record,
    sweep: {
      ...record.sweep,
      // Written before the sweep could run wide, so it did not.
      concurrency: upgradeConcurrency(record.sweep?.concurrency),
      unfixtured: record.sweep?.unfixtured ?? [],
    },
    // Older archives predate per-tier stamps; the run's own date is the honest
    // answer for every tier it contains.
    tierAt: record.tierAt ?? Object.fromEntries((record.tiers ?? []).map((t) => [t, record.at])),
    // Fields added to a CASE after archiving are read behind `?.` at every call
    // site (they are all optional detail), but the arrays themselves are not.
    cases: record.cases ?? [],
    harnesses: record.harnesses ?? [],
  }
}

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
  estimateProbes: (model: string, opts: { reprobe?: boolean }) => Promise<ProbeEstimate>
  runProbes: (model: string, opts: { reprobe?: boolean }) => Promise<ProbeReport>
  /** Throw away one candidate's resume ledger — see `clearFitnessResults`. */
  clearEvalStatus: (model: string) => Promise<void>
  /** Throw away archived transcripts; null means every model. Returns rows. */
  clearTranscripts: (model: string | null) => Promise<number>
  /** Registered MCP servers, for "the deployment supplies what the model
   *  cannot" — see `suppliedBy`. */
  mcpServers: () => Promise<McpServer[]>
  /** Talaria's OWN checked tools, under the registry. Injected rather than
   *  imported at the call site so a matrix test never has to reach SearXNG. */
  platformSupply: () => Promise<PlatformSupply[]>
  estimateAdversarial: (opts: {
    adversaryModel?: string
    price?: (promptTokens: number, completionTokens: number) => Promise<number | null>
  }) => Promise<AdversarialEstimate>
  runAdversarial: (model: string, opts: { adversaryModel?: string }) => Promise<AdversarialReport>
  runEvalSweep: (model: string, opts: { restart: boolean; only?: string[] }) => Promise<EvalSweep>
  evalSweepStatuses: (models: readonly string[]) => Promise<Record<string, EvalSweepStatus>>
  stopEvalSweep: (model?: string) => boolean
  guardConfig: () => Promise<GuardConfig>
  /** What this deployment can reach for a model — natively or by tool. */
  reach: (keys: readonly string[], wanted: readonly Capability[]) => Promise<Record<string, Reach>>
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
  estimateProbes: (model, opts) => estimateProbes(model, opts),
  runProbes: (model, opts) => runProbes(model, opts),
  clearEvalStatus,
  clearTranscripts,
  mcpServers: listMcpServers,
  platformSupply,
  estimateAdversarial: (opts) => estimateAdversarial(opts),
  runAdversarial: (model, opts) => runAdversarial(model, opts),
  runEvalSweep: (model, opts) =>
    runEvalSweep(model, {
      ...opts,
      shouldStop: (m) => stopRequestedFor(m),
      // EVERY CASE, PASSING INCLUDED — see `fitness/transcripts.ts` for why the
      // report's "keep it only if it failed" rule cannot answer the question an
      // audit asks.
      archiveCase: recordTranscript,
      archivePrune: (m) => pruneTranscripts(m).catch(() => {}),
    }),
  evalSweepStatuses,
  stopEvalSweep,
  guardConfig: getGuardConfig,
  reach: reachFor,
  observedHarnesses: (opts) => observedHarnesses(opts ?? {}),
  observedModels: () => observedModels(),
  nowIso: () => new Date().toISOString(),
}

const withDeps = (deps: Partial<SurfaceDeps> | undefined): SurfaceDeps => ({ ...REAL_DEPS, ...deps })

// ── Model rows and capability facts ──────────────────────────────────────────

/** FOUR STATES, and the fourth is a real one.
 *
 *  `supplied` means the MODEL cannot do it and the DEPLOYMENT can — a registered
 *  tool supplies it (`capability-reach.ts`). That is neither a yes nor a no and
 *  must not be collapsed into either: calling it 'yes' claims a blind model sees,
 *  and calling it 'no' refuses a deployment that can genuinely do the job. It is
 *  the distinction `capability-reach.ts` already draws with `via: 'native' |
 *  'tool'`, surfaced where an admin actually picks a model. */
export type CapabilityState = 'yes' | 'no' | 'unknown' | 'supplied'

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
  /** What supplies it, when `state` is 'supplied'. Named so the tag can say
   *  WHICH tool — "supplied" with no attribution is a claim an admin cannot
   *  check, and the supplier is the thing that might be switched off tomorrow. */
  via: { server: string; tool: string } | null
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
    return { state: 'unknown', source: null, detail: null, score: null, at: null, via: null }
  }
  const first = known[0]!
  if (!known.every((f) => f.value === first.value)) {
    return { state: 'unknown', source: null, detail: POOLED_DISAGREEMENT, score: null, at: null, via: null }
  }
  return {
    state: first.value ? 'yes' : 'no',
    source: first.source,
    detail: first.detail ?? null,
    score: first.score ?? null,
    at: first.at,
    via: null,
  }
}

/** THE DEPLOYMENT CAN, EVEN THOUGH THE MODEL CANNOT.
 *
 *  Applied after `mergeFact`, and only ever to a `no` or an `unknown`: a model
 *  that does the thing natively is not "supplied", and overwriting a measured
 *  `yes` would hide the fact that no tool is needed. Everything else is left
 *  exactly as measured — this promotes reach, it never invents a capability.
 *
 *  WHY IT IS NOT A `yes`. Calling it one claims a blind model sees, which is the
 *  false-true this whole capability model is built to avoid; calling it a `no`
 *  refuses a deployment that can genuinely do the job. It is a third fact and it
 *  gets a third tag. */
export const suppliedBy = (
  view: Omit<CapabilityView, 'cap'>,
  supplier: { server: string; tool: string } | null,
): Omit<CapabilityView, 'cap'> =>
  supplier && (view.state === 'no' || view.state === 'unknown')
    ? { ...view, state: 'supplied', via: supplier, detail: `the model does not do this itself; '${supplier.server}.${supplier.tool}' supplies it` }
    : view

/** The archive, re-keyed onto the ids the catalog now offers.
 *
 *  A report archived under `deepseek/deepseek-v4-flash` is a report about the
 *  deployment now called `openrouter/deepseek/deepseek-v4-flash` — same
 *  endpoint, same capability key, same weights. Left alone, that run's verdicts
 *  would light no cell and the admin would be asked to buy it again.
 *
 *  A canonical entry always WINS over a bare one that maps onto it: if both
 *  exist, the qualified id was tested more recently or more specifically, and
 *  in either case it is the one that named its endpoint. */
export function canonicalIndex(index: FitnessIndex, catalog: readonly GatewayModel[]): FitnessIndex {
  const out: FitnessIndex = {}
  for (const [model, entry] of Object.entries(index)) {
    const id = canonicalModelId(model, catalog)
    // `entry.model` KEEPS THE STORED SPELLING while the KEY becomes the
    // canonical one, and the distinction is load-bearing rather than tidy.
    // Re-keying the index moves where the page LOOKS a model up; it does not
    // move the archive, which still lives at `model_fitness_report:<the id the
    // run used>`. Overwriting `model` with the canonical id broke every reader
    // that goes on to fetch the report by it: the drill-down found no record
    // for a model it had just drawn a full row of verdicts for, and the value
    // view's backfill silently gave up and fell back to the shared token
    // budget. `storedIdFor` is the one way back.
    if (id === model || out[id] === undefined) out[id] = entry
  }
  // Second pass so an entry already under its canonical id is never displaced
  // by one that merely maps there.
  for (const [model, entry] of Object.entries(index)) if (catalog.some((m) => m.id === model)) out[model] = entry
  return out
}

/** THE ID THIS MODEL'S ARCHIVE IS FILED UNDER.
 *
 *  Not always the id the catalog now offers: a run archives under whatever id
 *  it was started with, and `canonicalIndex` re-keys the index onto the offered
 *  spelling without moving the report. Every reader that turns a row into a
 *  `recordKey` has to come through here. New runs archive under the canonical
 *  id already, so this stops mattering as old reports age out. */
export const storedIdFor = (model: string, index: FitnessIndex): string => index[model]?.model ?? model

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

  // WHAT THE DEPLOYMENT SUPPLIES, read once for the whole list. A registered
  // tool is a property of the install, not of a model, so asking per row would
  // be one listing per model for an answer that cannot differ between them.
  const servers = await d.mcpServers().catch((): McpServer[] => [])
  const providers = await d.readSetting<CapabilityProviders>(PROVIDERS_KEY, {}).catch((): CapabilityProviders => ({}))
  // Talaria's own checked tools, under the registry — so a `supplied` tag shows
  // on an install that has registered nothing but can still do the work.
  const platform = await d.platformSupply().catch((): PlatformSupply[] => [])
  const suppliers = new Map(CAPABILITY_ORDER.map((cap) => [cap, supplierFor(cap, servers, providers, platform)]))

  const rows: ModelRow[] = []
  for (const m of models) {
    const keys = keysFor(m)
    const perKey = await Promise.all(keys.map(factsFor))
    rows.push({
      id: m.id,
      qualified: m.qualified,
      endpoints: m.endpoints,
      pooled: !m.qualified && m.endpoints.length > 1,
      capabilities: CAPABILITY_ORDER.map((cap) => ({
        cap,
        ...suppliedBy(mergeFact(perKey.map((f) => f[cap])), suppliers.get(cap) ?? null),
      })),
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
  /** Mirrors `StartOptions.reprobe`, so the price shown is the price of the run
   *  the button will actually start. */
  reprobe?: boolean
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
    const est = await d.estimateProbes(model, { reprobe: req.reprobe === true }).catch(() => null)
    rows.push({
      tier: 'probes',
      calls: est?.calls ?? 0,
      promptTokens: est?.promptTokens ?? 0,
      completionTokens: est?.completionTokens ?? 0,
      usd: est?.usd ?? null,
      basis: 'fixture',
      note:
        (est?.known ?? 0) > 0
          ? `${est?.known} capability(ies) were already measured on this endpoint and are reused, not re-bought. Tick "re-measure capabilities" to pay for them again.`
          : 'Fixed prompts, so this is exact — except the long-context probe, which is sized from the model’s own advertised window.',
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
/** THE LIVE LIST, which is not the archived one.
 *
 *  `drilldown` keeps every clean case because they are cheap and an archive is
 *  read once. This is polled every three seconds while a sweep runs, and a clean
 *  case carries nothing the progress counter does not already say — so the live
 *  view sends what an admin opened the modal to watch: everything that FAILED
 *  something, plus the last few so the list visibly moves.
 *
 *  Measured on a real sweep: 155 cases a poll became 34. */
export const LIVE_RECENT = 6

/** ONE LINE PER CASE, FOR THE LIVE FEED.
 *
 *  `liveCases` above sends the failures plus a handful of recent ones, because a
 *  full `EvalCaseScore` carries a prompt, a reply and a tool log and a poll every
 *  three seconds cannot ship 250 of those. But an admin watching a sweep wants to
 *  SEE IT MOVING — which fixture is running, what each one decided, how long it
 *  took — and a list that only shows failures looks identical whether the sweep
 *  is flying or wedged.
 *
 *  So this is the other half: every case that has landed, reduced to the fields a
 *  terminal line needs. ~90 bytes each, so the whole 250-fixture sweep is about
 *  20KB — an order of magnitude under what one failed case's transcript costs.
 *
 *  NEWEST LAST, because it is a log and a log reads downward. */
export interface EvalLogLine {
  harness: string
  case: string
  /** What happened, in the vocabulary the terminal colours by. `gap` is OURS —
   *  the fixture could not fairly ask its question — and is deliberately its own
   *  verdict rather than folded into `fail`. */
  verdict: 'pass' | 'fail' | 'gap' | 'skip' | 'timeout' | 'error'
  ms: number
  tokens: number
  /** Tool calls the case made, for a dry run. Zero elsewhere. */
  calls: number
  /** UPSTREAM CALLS the case made, and how many never came back. The two
   *  numbers that turn a timeout from a symptom into a diagnosis: `1/1 open`
   *  is a request that hung, `4/0 open` is a case that spent its budget on
   *  retries, and `0` is time that went somewhere before the provider. */
  up: { calls: number; open: number } | null
  /** The fixture's own sentence, when there is one to show. */
  note: string | null
}

/** Lines kept in the feed. A sweep is ~250 fixtures, so this holds an entire run
 *  and only bites on a resumed sweep that has already run several times. */
export const LIVE_LOG_CAP = 400

export function liveLog(cases: readonly EvalCaseScore[]): EvalLogLine[] {
  return cases.slice(-LIVE_LOG_CAP).map((c) => ({
    harness: c.harness,
    case: c.case,
    verdict:
      c.skipped !== null
        ? 'skip'
        : c.timedOut
          ? 'timeout'
          : c.gap !== null
            ? 'gap'
            : !c.contractHeld || c.error !== null
              ? 'error'
              : c.task === 'fail'
                ? 'fail'
                : 'pass',
    ms: c.latencyMs,
    tokens: c.promptTokens + c.completionTokens,
    calls: c.calls?.length ?? 0,
    up: c.upstream?.length ? { calls: c.upstream.length, open: c.upstream.filter((u) => !u.settled).length } : null,
    // The reason, whichever kind it is, capped to a terminal line. `error` first:
    // when a case both errored and failed its check, the error is the cause.
    note: (c.error ?? c.gap ?? c.taskError ?? c.skipped)?.slice(0, 200) ?? null,
  }))
}

export function liveCases(cases: readonly EvalCaseScore[]): { kept: EvalCaseScore[]; dropped: number } {
  const bad = (c: EvalCaseScore): boolean => c.skipped === null && (c.task === 'fail' || !c.contractHeld || c.timedOut)
  const failed = cases.filter(bad)
  const recent = cases.slice(-LIVE_RECENT).filter((c) => !bad(c))
  const kept = [...failed.slice(-DRILLDOWN_CAP), ...recent]
  return { kept, dropped: cases.length - kept.length }
}

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
  /** The reading the last run left behind. Carried so a pass that measured
   *  NOTHING — a probes-only run, a sweep the admin stopped at case one — keeps
   *  the previous number instead of blanking a column somebody paid for. */
  previousSpeed?: SpeedReading | null
}

/** The matrix row, assembled from what the run produced. */
export function indexEntryOf(parts: IndexEntryParts): FitnessIndexEntry {
  const { sweep, adversarial, probes } = parts
  const cells: FitnessIndexEntry['cells'] = {}
  for (const slot of parts.report.slots) {
    cells[slotKey(slot.slot)] = { band: slot.band, reason: slot.reasons[0]?.detail ?? null }
  }
  // NULL UNLESS EVERY COMPONENT THAT SPENT ANYTHING PRICED. A partial total
  // under a dollar sign is a number nobody can reconcile with the invoice, which
  // is worse than no number — that half is unchanged.
  //
  // WHAT CHANGED IS WHAT COUNTS AS A COMPONENT. A harness whose cases were all
  // SKIPPED reports `costUsd: null` because it priced nothing — and it spent
  // nothing, so it is free rather than unpriced. Treating those two the same
  // meant one skipped harness turned a fully-priced run into "unpriced" in the
  // modal header, and the qwen run — where a routing refusal skipped everything
  // — made it obvious. A harness that burned tokens and could not be priced
  // still poisons the total, which is the case the rule was written for.
  const spent = (h: { costUsd: number | null; promptTokens: number; completionTokens: number }): boolean =>
    h.costUsd !== null || h.promptTokens > 0 || h.completionTokens > 0
  const billed = [...sweep.harnesses.filter(spent), ...(adversarial && spent(adversarial) ? [adversarial] : [])]
  const costUsd = billed.length === 0 || billed.some((p) => p.costUsd === null) ? null : billed.reduce<number>((n, p) => n + (p.costUsd ?? 0), 0)

  return {
    model: parts.model,
    at: parts.at,
    tiers: parts.ran,
    guarded: sweep.guarded,
    cells,
    safety: adversarial ? { band: adversarial.band, resistance: adversarial.resistance } : null,
    probesWrote: probes?.wrote ?? 0,
    // FROM THE PASS THAT JUST RAN. A supplemental or speed-only pass refreshes
    // the reading without re-buying the battery; a pass that ran nothing (probes
    // only) leaves the previous reading alone rather than nulling it.
    speed: speedOf(sweep.measured, sweep.concurrency.ended) ?? parts.previousSpeed ?? null,
    costUsd,
    calls: sweep.done + (adversarial?.cases.length ?? 0) + (probes?.results.length ?? 0),
    // Partial in either of the two ways a run can be: tier 2 stopped
    // mid-sweep, or a tier the admin asked for never produced a result.
    partial: sweep.state === 'stopped' || (sweep.total > 0 && sweep.done < sweep.total) || parts.ran.length < parts.requested.length,
    // The per-harness half, from the one definition the backfill also uses.
    harnesses: harnessSummary(parts.report, sweep.harnesses),
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
  // No cases ran, so there is no width to report and 1 is the honest reading.
  concurrency: { requested: 1, ended: 1, low: 1, narrowedBecause: null },
  measured: [],
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

/** The budget a sweep leaves behind. Pure, and separated from the read/write
 *  for the reason the rule below is worth a test of its own. */
export function nextBudget(prev: TokenBudget, harnesses: readonly HarnessScore[], at: string): TokenBudget {
  const budget: TokenBudget = { ...prev }
  for (const h of harnesses) {
    if (h.cases === 0) continue
    // ZERO TOKENS IS NOT A MEASUREMENT OF ZERO, and writing it as one cost a
    // real install its whole budget: a sweep against a model id the gateway
    // could not reach ran all 70 cases, failed every one before a single token
    // moved, and overwrote 26 harnesses' good numbers with 0. Every dollar
    // figure downstream — the tier-2 estimate, the value view's daily cost —
    // then read $0.00 for every model on the page. A run that measured nothing
    // leaves the previous measurement where it is.
    if (h.promptTokens + h.completionTokens === 0) continue
    budget[h.id] = { prompt: Math.round(h.promptTokens / h.cases), completion: Math.round(h.completionTokens / h.cases), at }
  }
  return budget
}

async function recordBudget(harnesses: readonly HarnessScore[], d: SurfaceDeps): Promise<void> {
  if (harnesses.length === 0) return
  const prev = await d.readSetting<TokenBudget>(BUDGET_KEY, {})
  await d.writeSetting(BUDGET_KEY, nextBudget(prev, harnesses, d.nowIso())).catch(() => {})
}

// ── The run ──────────────────────────────────────────────────────────────────

/** HOW MANY CANDIDATES MAY BE TESTED AT ONCE.
 *
 *  EIGHT, RAISED FROM THREE once the checkpoint stopped being shared. Three was
 *  never about the provider — each run is strictly sequential inside itself (one
 *  harness, then one case, then the next), so N runs are N concurrent requests
 *  and eight of those is nothing. It was about US: the resume checkpoint used to
 *  live in one settings row holding every running candidate's cases, so each
 *  per-case write was a read-modify-write of every sibling's work too. Write
 *  traffic went as O(N² x cases²) — ~400 MB at three candidates, ~4.6 GB at ten
 *  — and each write was a synchronous JSON parse/stringify of a multi-megabyte
 *  blob on the event loop serving the UI. Five or six was the real ceiling.
 *
 *  One row per candidate (`harness_eval_run:<model>`) took N out of the cost
 *  entirely: a write is proportional to one candidate's own cases and touches
 *  nothing else, so eight concurrent sweeps write exactly what eight sequential
 *  ones would.
 *
 *  WHAT BINDS NOW IS THE PROVIDER, not this process. Measured at ~16k tokens a
 *  minute per run, so eight is ~130k TPM against one key — comfortably inside a
 *  paid tier and the point where a single key starts to throttle. Candidates
 *  spread across endpoints do not share that budget at all, so a fleet with
 *  several providers can go higher; raise it here if that is the shape of the
 *  install. */
export const MAX_CONCURRENT_RUNS = 8

/** The candidates running IN THIS PROCESS, each with its own stop flag. Stop is
 *  honored BETWEEN tiers as well as inside tier 2 — `stopEvalSweep` only
 *  reaches the sweep, and a run stopped during the probes would otherwise go on
 *  to buy the whole tier-2 sweep the admin just asked it not to. */
const runs = new Map<string, { stop: boolean }>()

export const runningModels = (): string[] => [...runs.keys()]

/** Why a run cannot start, or null. Separated from the claim so the route can
 *  say WHICH of the two refusals it hit. */
export type RunRefusal = 'already-running' | 'at-capacity'

/** Claim a run slot SYNCHRONOUSLY. Two simultaneous Start presses both clear an
 *  `if (running)` written above an `await` — the check and the claim have to be
 *  one step, and the caller owns releasing it (the `finally` in `runFitness`). */
function claimRun(model: string): RunRefusal | null {
  if (runs.has(model)) return 'already-running'
  if (runs.size >= MAX_CONCURRENT_RUNS) return 'at-capacity'
  runs.set(model, { stop: false })
  return null
}

export interface StartOptions {
  model: string
  tiers: TierId[]
  adversaryModel: string | null
  only?: string[]
  restart: boolean
  /** Re-measure capabilities we already have a probe fact for. Off by default —
   *  see `runProbes`. The release valve for a model id that was re-pointed at
   *  different weights is "Forget recorded capabilities"; this is the softer one
   *  for an admin who just wants a fresh reading. */
  reprobe?: boolean
  /** Cases in flight at once. See `DEFAULT_CONCURRENCY` in evals.ts — and note
   *  it multiplies with `MAX_CONCURRENT_RUNS`. */
  concurrency?: number
  /** Keep the passes, re-ask everything else. See `EvalOptions.retryFailed`. */
  retryFailed?: boolean
  /** Run only fixtures that have never been run. See `EvalOptions.supplement`. */
  supplement?: boolean
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
  /** Both halves: the in-process flag for a run this instance holds, and the
   *  persisted request for everything else. Checked between tiers, which is
   *  where `runFitness` honors a stop. */
  const stopped = async (): Promise<boolean> => (runs.get(model)?.stop ?? false) || (await stopRequestedFor(model, d).catch(() => false))
  const writeStatus = (s: FitnessRunStatus): Promise<void> => writeRunStatus(d, s)
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
    if (tiers.includes('probes') && !(await stopped())) {
      await setPhase('probes')
      probes = await d.runProbes(model, { reprobe: opts.reprobe === true }).catch(() => null)
      if (probes) ran.push('probes')
    }

    const harnesses = await d.harnesses()
    let sweep: EvalSweep | null = null
    if (tiers.includes('evals') && !(await stopped())) {
      await setPhase('evals')
      sweep = await d
        .runEvalSweep(model, {
          restart: opts.restart,
          ...(opts.retryFailed ? { retryFailed: true } : {}),
          ...(opts.supplement ? { supplement: true } : {}),
          ...(opts.concurrency !== undefined ? { concurrency: opts.concurrency } : {}),
          ...(opts.only?.length ? { only: opts.only } : {}),
        })
        .catch(() => null)
      if (sweep) {
        await recordBudget(sweep.harnesses, d)
        ran.push('evals')
      }
    }

    let adversarial: AdversarialReport | null = null
    if (tiers.includes('adversarial') && !(await stopped())) {
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
    // A TIER THAT DID NOT RUN KEEPS ITS LAST RESULT. IT DOES NOT GET BLANKED.
    //
    // THIS DESTROYED PAID-FOR WORK. The record was assembled from THIS run
    // alone, so an adversarial-only run rebuilt it with `emptySweep` — every
    // bound slot went `untested`, every fixture case vanished, and the only way
    // back was to buy the whole sweep again. It ran in all three directions: a
    // probes-only run erased tier 2 and tier 3, an evals-only run erased the
    // adversarial band an admin had just paid for.
    //
    // Re-running ONE tier is the normal thing to want — a fixture changed, the
    // provocation corpus grew, a capability needs re-measuring — so the merge is
    // the behaviour, not an option. `tierAt` records when each part was last
    // measured, because a report that carries a month-old probe result must be
    // able to say so rather than showing today's date over all of it.
    const prior = upgradeRecord(await d.readSetting<FitnessRecord | null>(recordKey(storedIdFor(model, await readIndex(d))), null).catch(() => null))
    const carriedSweep: EvalSweep | null =
      prior && prior.harnesses.length > 0
        ? {
            ...emptySweep(model, prior.sweep.unfixtured, prior.report.guarded, prior.at),
            state: prior.sweep.state as EvalSweep['state'],
            done: prior.sweep.done,
            total: prior.sweep.total,
            harnesses: prior.harnesses,
            cases: prior.cases,
            concurrency: prior.sweep.concurrency,
          }
        : null
    const effective = sweep ?? carriedSweep ?? emptySweep(model, unfixtured, guarded, startedAt)
    const observed = await d.observedHarnesses().catch((): ObservedHarness[] => [])
    const rows = await modelRows(d).catch((): ModelRow[] => [])
    // WHAT THE DEPLOYMENT REACHES, asked once for every capability any bound
    // harness or slot requires. Without it a slot verdict is a statement about
    // the model alone, and the thing an admin assigns is a model running inside
    // Talaria with the tools this org registered — see `capability-reach.ts`.
    // A failure here degrades to the raw capability facts, which is the verdict
    // this page gave before reach existed: narrower, never wrong in the unsafe
    // direction.
    const wanted = [...new Set([...harnesses.flatMap((h) => h.requires), ...fitnessSlots().flatMap((sl) => sl.requires)])]
    const reach = await d.reach(await capabilityKeysFor(model).catch((): string[] => []), wanted).catch((): Record<string, Reach> => ({}))

    const report = scoreFitness(
      {
        sweep: effective,
        harnesses,
        capabilities: capabilitiesOf(rows.find((r) => r.id === model)),
        reach,
        guardBaseline: guardBaseline(observed),
      },
      await d.bindSlots(harnesses),
    )

    const at = d.nowIso()
    const { kept, dropped } = drilldown(effective.cases)
    // The tiers this RECORD now speaks for, which is what an admin reads — not
    // the tiers this run happened to buy.
    const carriedTiers: TierId[] = TIER_IDS.filter((t) => ran.includes(t) || (prior?.tiers.includes(t) ?? false))
    const record: FitnessRecord = {
      model,
      at,
      tiers: carriedTiers,
      // WHEN EACH PART WAS LAST MEASURED. A merged record shows today's date; a
      // probe result inside it may be a month old, and a report that cannot say
      // so is a report that quietly ages into a lie.
      tierAt: { ...(prior?.tierAt ?? {}), ...Object.fromEntries(ran.map((t) => [t, at])) },
      report,
      harnesses: effective.harnesses,
      cases: kept,
      droppedCases: dropped,
      probes: probes ?? prior?.probes ?? null,
      // A dozen provocations, of which only the ones the model FELL for carry
      // a transcript. Kept whole: this is the tier whose drill-down an admin is
      // most likely to need in order to justify a decision.
      adversarial: adversarial ?? prior?.adversarial ?? null,
      sweep: {
        state: effective.state,
        done: effective.done,
        total: effective.total,
        error: effective.error,
        unfixtured: effective.unfixtured,
        concurrency: effective.concurrency,
      },
    }
    await d.writeSetting(recordKey(model), record)

    const priorEntry = (await readIndex(d).catch((): FitnessIndex => ({})))[storedIdFor(model, await readIndex(d).catch((): FitnessIndex => ({})))]
    const entry = indexEntryOf({
      model,
      at,
      ran,
      requested: tiers,
      sweep: effective,
      report,
      probes: probes ?? prior?.probes ?? null,
      adversarial: adversarial ?? prior?.adversarial ?? null,
      previousSpeed: priorEntry?.speed ?? null,
    })
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
    runs.delete(model)
    // The request is spent once the run is over; leaving it would stop the next
    // Start before it began.
    await clearStopRequest(model, d).catch(() => {})
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

/** THE MATRIX'S COLUMNS — the slots a run can actually say something about.
 *
 *  A SLOT NOTHING REACHES IS NOT A COLUMN. Five of the twenty slots have no
 *  harness bound to them and never will as things stand, so every cell in them
 *  reads `unbound` for every model, forever. Five dead columns in a table an
 *  admin already has to scroll sideways is not caution, it is noise — and worse,
 *  it inflates the "untested" count in every row summary with slots that were
 *  never testable.
 *
 *  They are dead for TWO DIFFERENT REASONS, which the `live` flag flattens and
 *  the old "(reserved)" chip therefore mislabelled:
 *
 *    RESERVED ROLES (`wired: false`) — vision, image-generation, embedding,
 *    reranker. The surfaces do not exist yet. "Reserved" is right for these.
 *
 *    THE BRIEFER (`assignable: false`) — which is NOT reserved. It ships, it
 *    runs on every Inbox and console briefing, and it has harnesses. It has no
 *    column because its model is fixed BY DESIGN: a briefing reads one person's
 *    own views and answers in their own assistant's voice, so it resolves to the
 *    caller's personal assistant and there is nothing for an admin to assign.
 *    Its harnesses take their model from the subject of the call, which is why
 *    they land in `FitnessReport.unbound` — where they are still scored, and
 *    still shown in that model's report.
 *
 *  NOTHING STOPS BEING MEASURED. `fitnessSlots()` is unchanged, so `scoreFitness`
 *  still produces a verdict for all twenty and the archive still stores them;
 *  `roleAssignmentIssues` still warns an admin who assigns a blind model to the
 *  reserved vision role, which is the surface where that warning belongs. This
 *  decides what gets a column, and nothing else. */
export const slotViews = (): SlotView[] =>
  fitnessSlots()
    .filter((s) => s.live)
    .map((s) => ({ ...s, key: slotKey(s), taskFloor: taskFloorFor(s) }))

/** Every run's status, from the map plus the legacy single row.
 *
 *  THE LEGACY ROW IS FOLDED IN, NOT MIGRATED. It is the status of whatever run
 *  was in flight across the change to concurrent runs — code already loaded in
 *  the process keeps writing it, and the alternative to reading it is a run the
 *  admin can watch start and then never see finish. It loses to a real entry
 *  for the same model, and nothing writes it again. */
/** One run's status into the map. SERIALIZED, because three runs write their
 *  phase transitions concurrently and `setSetting` upserts the whole row — the
 *  last writer of a tick would otherwise drop its siblings' progress. */
let statusQueue: Promise<void> = Promise.resolve()

const writeRunStatus = (d: SurfaceDeps, status: FitnessRunStatus): Promise<void> =>
  (statusQueue = statusQueue
    .then(async () => {
      if (!status.model) return
      const runs = await d.readSetting<Record<string, FitnessRunStatus>>(RUNS_KEY, {})
      await d.writeSetting(RUNS_KEY, { ...runs, [status.model]: status })
    })
    .catch(() => {}))

async function readRuns(d: SurfaceDeps): Promise<Record<string, FitnessRunStatus>> {
  const [runs, legacy] = await Promise.all([
    d.readSetting<Record<string, FitnessRunStatus>>(RUNS_KEY, {}),
    d.readSetting<FitnessRunStatus | null>(STATUS_KEY, null),
  ])
  if (legacy?.model && runs[legacy.model] === undefined) return { ...runs, [legacy.model]: legacy }
  return runs
}

/** Merge one run's persisted status with its tier-2 case counter. The counter
 *  belongs to tier 2 and is READ from it, never mirrored — two progress
 *  counters for one run is how they come to disagree. */
const statusView = (status: FitnessRunStatus, sweeps: Record<string, EvalSweepStatus>): FitnessStatusView => {
  const sweep = status.model ? sweeps[status.model] : undefined
  // ONLY WHEN THIS RUN IS ACTUALLY SWEEPING. The checkpoint is per model and
  // outlives the run that wrote it, so a probes-only run on a model swept
  // earlier displayed that older sweep's counter — "probes 247/247" on a run
  // with no fixtures in it at all.
  const live = status.state === 'running' && sweep !== undefined && status.tiers.includes('evals')
  return {
    ...status,
    done: live ? sweep.done : 0,
    total: live ? sweep.total : 0,
    harness: live ? sweep.harness : null,
    sweepState: sweep?.state ?? 'idle',
  }
}

/** ONE candidate's status, or the idle row. Kept because three callers ask
 *  about a specific model and would otherwise each filter the list. */
export async function fitnessStatus(model: string | null, deps?: Partial<SurfaceDeps>): Promise<FitnessStatusView> {
  const all = await fitnessRuns(deps)
  return all.runs.find((r) => r.model === model) ?? { ...IDLE, done: 0, total: 0, harness: null, sweepState: 'idle' }
}

export async function fitnessRuns(deps?: Partial<SurfaceDeps>): Promise<FitnessRunsView> {
  const d = withDeps(deps)
  // The checkpoint rows are asked for BY NAME, from the models the status store
  // already knows about — one small row each, rather than one shared blob that
  // grew with every case of every concurrent run.
  const statuses = await readRuns(d)
  const sweeps = await d.evalSweepStatuses(Object.keys(statuses)).catch((): Record<string, EvalSweepStatus> => ({}))
  const runs = Object.values(statuses)
    .map((s) => statusView(s, sweeps))
    // Running first, then most recently started — an admin watching three
    // sweeps wants the live ones at the top, not whichever id sorts first.
    .sort((a, b) => {
      const live = Number(b.state === 'running') - Number(a.state === 'running')
      return live !== 0 ? live : (b.startedAt ?? '').localeCompare(a.startedAt ?? '')
    })
  const inFlight = runs.filter((r) => r.state === 'running').length
  return { runs, max: MAX_CONCURRENT_RUNS, full: inFlight >= MAX_CONCURRENT_RUNS }
}

/** The value view's edges, bound to this module's injected ones so a test can
 *  drive the whole page from `SurfaceDeps` and never learn a second deps shape.
 *  `value.ts` itself stays free of every real import — it is arithmetic over
 *  what it is handed. */
/** The archive as every READER wants it — re-keyed onto the ids the catalog
 *  offers. The writers (`runFitness`, `evictArchive`, `forgetModel`) keep
 *  reading it raw: they operate on stored state, and re-keying under them would
 *  archive a run twice or delete the wrong row. */
const readIndex = async (d: SurfaceDeps): Promise<FitnessIndex> => {
  const [index, catalog] = await Promise.all([d.readSetting<FitnessIndex>(INDEX_KEY, {}), d.models().catch((): GatewayModel[] => [])])
  return catalog.length === 0 ? index : canonicalIndex(index, catalog)
}

export const readValue = (d: SurfaceDeps): Promise<ValueView> =>
  valueView({
    observed: () => d.observedHarnesses(),
    harnesses: d.harnesses,
    bindings: d.bindSlots,
    index: () => readIndex(d),
    budget: () => d.readSetting<TokenBudget>(BUDGET_KEY, {}),
    price: (model) => priceOf(model, d),
    record: async (model) => upgradeRecord(await d.readSetting<FitnessRecord | null>(recordKey(model), null)),
    windowDays: DEFAULT_WINDOW_DAYS,
  })

export interface MatrixView {
  slots: SlotView[]
  models: ModelRow[]
  index: FitnessIndex
  /** EVERY RUN, not one. Up to `max` candidates are tested at once, and a
   *  single `status` field could only ever draw one of them — which is how an
   *  admin who started three would watch two of them vanish. */
  runs: FitnessStatusView[]
  max: number
  full: boolean
  thresholds: typeof THRESHOLDS
  registry: { harnesses: number; fixtures: number; provocations: number; unfixtured: string[] }
}

export interface CapabilitiesView {
  models: ModelRow[]
  index: FitnessIndex
}

export interface EstimateView {
  estimate: RunEstimate
  adversaryRequirement: typeof ADVERSARY_REQUIREMENT
}

/** A RUN IN FLIGHT, as the drill-down can show it.
 *
 *  The sweep checkpoints every case as it lands, so the audit trail an admin
 *  wants while a run is working already exists — it was simply never sent. The
 *  drill-down read only the ARCHIVED record, so opening a model mid-run said
 *  "no run on record" about a sweep that was at that moment 140 fixtures in.
 *
 *  Bounded by the same `drilldown` the archive uses: failures first, capped, and
 *  it says how many it dropped. A checkpoint runs to hundreds of kilobytes and
 *  this is polled. */
export interface LiveRun {
  state: string
  phase: TierId | 'scoring' | null
  done: number
  total: number
  /** The harness being swept right now, for the line above the list. */
  harness: string | null
  cases: EvalCaseScore[]
  dropped: number
  /** THE FEED: one line per landed case, every case, cheap. `cases` above is the
   *  drill-down sample; this is the thing that shows a sweep moving. */
  log: EvalLogLine[]
  /** THE CASES RUNNING RIGHT NOW, with their turns as they happen. Empty when
   *  the sweep belongs to another instance — see `inFlightFor`. That reads as an
   *  empty panel, never as a wrong one. Several at once: a sweep runs
   *  `concurrency` cases in parallel. */
  current: InFlightCase[]
}

export interface DetailView {
  model: string
  record: FitnessRecord | null
  /** Non-null only while this candidate is being tested. */
  live: LiveRun | null
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
  /** `?reprobe=1` — price the run that RE-MEASURES capabilities we already have,
   *  so the estimate matches the box the admin just ticked. */
  reprobe?: boolean
  /** `?run=<iso>` — which archived run's transcripts to read. Omitted means the
   *  newest one on record. */
  run?: string | null
}

export interface TranscriptView {
  model: string
  runs: Array<{ runStartedAt: string; cases: number }>
  cases: Transcript[]
}

export type FitnessGetResult =
  | { ok: true; body: MatrixView | CapabilitiesView | EstimateView | DetailView | ValueView | TranscriptView | HealthSummary }
  /** A 400 with a sentence. The route maps it; nothing here knows about codes. */
  | { ok: false; error: string }

export async function readFitness(query: FitnessQuery, deps?: Partial<SurfaceDeps>): Promise<FitnessGetResult> {
  const d = withDeps(deps)
  const { model } = query

  if (query.view === 'capabilities') {
    // The index rides along: the panels that PICK a model need the band its
    // last run gave the slot they are assigning, and a second round trip for
    // two dozen small cell maps would be a request per panel per open.
    return { ok: true, body: { models: await modelRows(d), index: await readIndex(d) } }
  }

  if (query.view === 'transcripts') {
    // THE AUDIT VIEW, and it is its own request on purpose. A full run is every
    // case with its prompt, its reply, its turns and its tool calls — hundreds of
    // kilobytes — and the fitness page is polled every three seconds while a
    // sweep is in flight. Nothing that renders by default may touch this.
    if (!model) return { ok: false, error: 'model is required' }
    return {
      ok: true,
      body: {
        model,
        runs: await transcriptRuns(model),
        cases: await readTranscripts(model, query.run ?? undefined),
      },
    }
  }

  if (query.view === 'health') {
    // ACROSS EVERY ARCHIVED MODEL, which is the only way to tell a broken
    // fixture from a hard one. Its own request: it reads one record per tested
    // candidate, and the matrix is polled every three seconds while a run is in
    // flight.
    const index = await readIndex(d)
    const runs: HealthInput[] = []
    for (const id of Object.keys(index)) {
      const rec = upgradeRecord(await d.readSetting<FitnessRecord | null>(recordKey(id), null).catch(() => null))
      if (rec) runs.push({ model: rec.model, cases: rec.cases })
    }
    return { ok: true, body: summarize(runs) }
  }

  if (query.view === 'value') {
    // Its own view, not a field on the matrix: it costs a telemetry query and a
    // price lookup per tested model, and the matrix is polled every 3s while a
    // run is in flight. An admin opens this one on purpose.
    return { ok: true, body: await readValue(d) }
  }

  if (query.view === 'estimate') {
    if (!model) return { ok: false, error: 'model is required' }
    const tiers = (query.tiers ?? 'probes,evals').split(',').filter(isTierId)
    if (tiers.length === 0) return { ok: false, error: 'pick at least one tier' }
    const only = query.only?.split(',').filter(Boolean)
    return {
      ok: true,
      body: {
        estimate: await estimateRun({ model, tiers, adversaryModel: query.adversary, reprobe: query.reprobe === true, ...(only ? { only } : {}) }, d),
        adversaryRequirement: ADVERSARY_REQUIREMENT,
      },
    }
  }

  if (query.view === 'detail') {
    if (!model) return { ok: false, error: 'model is required' }
    // Through `storedIdFor`, because the row an admin clicked is keyed by the
    // id the CATALOG offers and the report is filed under the id the RUN used.
    // Reading it directly showed "no run on record" for a model whose verdicts
    // were on screen a click earlier.
    const record = upgradeRecord(await d.readSetting<FitnessRecord | null>(recordKey(storedIdFor(model, await readIndex(d))), null))
    // Production telemetry is ADVISORY and is fetched even with no bench
    // record: "this model is running in production and has never been
    // tested" is one of the more useful things this page can say.
    const [observed, models, runs] = await Promise.all([
      d.observedHarnesses({ model }).catch((): ObservedHarness[] => []),
      d.observedModels().catch((): ObservedModel[] => []),
      fitnessRuns(d).catch((): FitnessRunsView => ({ runs: [], max: MAX_CONCURRENT_RUNS, full: false })),
    ])

    // THE LIVE HALF. Only while this candidate is actually running: a finished
    // run has an archived record, which is the better thing to read, and a
    // checkpoint outlives the run that wrote it.
    const running = runs.runs.find((r) => r.model === model && r.state === 'running')
    let live: LiveRun | null = null
    if (running) {
      const sweep = await d.evalSweepStatuses([model]).catch((): Record<string, EvalSweepStatus> => ({}))
      const cases = sweep[model]?.cases ?? []
      const kept = liveCases(cases)
      live = {
        state: running.state,
        phase: running.phase,
        done: running.done,
        total: running.total,
        harness: running.harness,
        cases: kept.kept,
        dropped: kept.dropped,
        log: liveLog(cases),
        current: inFlightFor(model),
      }
    }

    return {
      ok: true,
      body: {
        model,
        record,
        live,
        observed,
        observedModel: models.find((m) => m.model === model) ?? null,
        divergences: record ? divergences(model, record.harnesses, observed) : [],
        thresholds: THRESHOLDS,
      },
    }
  }

  const [models, index, runsView, shape] = await Promise.all([modelRows(d), readIndex(d), fitnessRuns(d), tier2Shape(undefined, d)])
  return {
    ok: true,
    body: {
      slots: slotViews(),
      models,
      index,
      // The runs the page draws, plus the cap it disables Start at. Three at
      // once is the feature; one `status` field could only ever show one.
      runs: runsView.runs,
      max: runsView.max,
      full: runsView.full,
      thresholds: THRESHOLDS,
      registry: {
        harnesses: shape.harnesses.length,
        fixtures: shape.fixtures,
        // TIER 3 IS PART OF THE BATTERY AND WAS NOT PART OF THE COUNT. A page
        // reading "26 harnesses · 247 fixtures" over a suite that also runs 19
        // safety provocations understates what a full run measures, and the
        // adversarial corpus is the half an admin is least likely to know about.
        provocations: SEEDS.length,
        unfixtured: shape.harnesses.filter((h) => h.evalNames.length === 0).map((h) => h.id),
      },
    },
  }
}

// ── The write verbs ──────────────────────────────────────────────────────────

export type StartResult =
  /** 409. Either this candidate is already running — the second press of Start
   *  means "show me the run", not "start a second one", the same call the
   *  tier-2 sweep makes for itself — or every slot is taken. `refusal` tells
   *  the two apart so the route can say which. */
  | { ok: false; reason: 'busy'; refusal: RunRefusal; status: FitnessStatusView; runs: FitnessRunsView }
  /** 400, with the sentence the admin reads. */
  | { ok: false; reason: 'rejected'; error: string }
  | { ok: true; status: FitnessStatusView; runs: FitnessRunsView }

/** Validate, claim the run slot, and detach the run. */
export async function startFitnessRun(
  req: {
    model: string
    tiers: TierId[]
    adversaryModel: string | null
    only?: string[]
    restart: boolean
    reprobe?: boolean
    concurrency?: number
    retryFailed?: boolean
    supplement?: boolean
  },
  deps?: Partial<SurfaceDeps>,
): Promise<StartResult> {
  const d = withDeps(deps)
  // A fast 409 for the common case, before the catalog reads. Not the door —
  // `claimRun` below is, because only it is synchronous with the claim.
  const early = runs.has(req.model) ? 'already-running' : runs.size >= MAX_CONCURRENT_RUNS ? 'at-capacity' : null
  if (early) return { ok: false, reason: 'busy', refusal: early, status: await fitnessStatus(req.model, d), runs: await fitnessRuns(d) }

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
    ...(req.reprobe ? { reprobe: true } : {}),
    ...(req.concurrency !== undefined ? { concurrency: req.concurrency } : {}),
    ...(req.retryFailed ? { retryFailed: true } : {}),
    ...(req.supplement ? { supplement: true } : {}),
    ...(req.only?.length ? { only: req.only } : {}),
  }
  // Claimed here, with no await between the check and the claim.
  const refusal = claimRun(opts.model)
  if (refusal) return { ok: false, reason: 'busy', refusal, status: await fitnessStatus(opts.model, d), runs: await fitnessRuns(d) }
  // A STOP REQUEST BELONGS TO THE RUN IT STOPPED. Clearing it only when a run
  // ENDS leaves it set for any run that never got to finish — a stopped sweep, a
  // process killed mid-run — and the next Start on that model then stops itself
  // after one case, having read a flag meant for a run that is already gone.
  // Observed exactly that: a fresh sweep recorded `state: 'stopped'` at 1/247.
  await clearStopRequest(opts.model, d).catch(() => {})
  await writeRunStatus(d, {
    state: 'running',
    model: opts.model,
    tiers: opts.tiers,
    phase: opts.tiers[0] ?? null,
    startedAt: d.nowIso(),
  } satisfies FitnessRunStatus)
  void runFitness(opts, deps).catch(() => {})
  return { ok: true, status: await fitnessStatus(opts.model, d), runs: await fitnessRuns(d) }
}

/** Stop one candidate's run, or — with no model — every run in flight.
 *
 *  TWO STOPS PER RUN, because there are two things running: the tier-2 sweep,
 *  which honors it at a case boundary and stays RESUMABLE, and the tier loop,
 *  which honors it by not buying the tiers that have not started. */
export async function stopFitnessRun(
  model: string | null,
  deps?: Partial<SurfaceDeps>,
): Promise<{ stopped: boolean; status: FitnessStatusView; runs: FitnessRunsView }> {
  const d = withDeps(deps)
  // THE TARGETS COME FROM THE PERSISTED STATUS, not from the in-process map.
  // That map is empty in any instance that did not start the run, which is
  // exactly the case where Stop was doing nothing at all.
  const live = (await fitnessRuns(d)).runs.filter((r) => r.state === 'running').map((r) => r.model)
  const targets = (model === null ? live : [model]).filter((m): m is string => m !== null)

  // Written FIRST, so the request outlives this process whatever happens next.
  if (targets.length > 0) {
    const asked = await d.readSetting<string[]>(STOP_KEY, []).catch((): string[] => [])
    await d.writeSetting(STOP_KEY, [...new Set([...asked, ...targets])]).catch(() => {})
    // The instance that TOOK the request must not then serve a cached "no" to
    // its own sweep for the next two seconds.
    stopCache = { at: 0, models: new Set() }
  }

  // Then the fast path, for a run this instance is holding.
  for (const m of targets) {
    const slot = runs.get(m)
    if (slot) slot.stop = true
    d.stopEvalSweep(m)
  }

  // `stopped` is about the REQUEST landing, not about which instance owns the
  // run: a button that reported false while the sweep obediently stopped was
  // the other half of this bug.
  return { stopped: targets.length > 0, status: await fitnessStatus(model, d), runs: await fitnessRuns(d) }
}

/** Has anyone asked this candidate to stop? Cached briefly because the sweep
 *  asks between every case and the answer changes about once a run. */
let stopCache: { at: number; models: Set<string> } = { at: 0, models: new Set() }
const STOP_TTL_MS = 2_000

export async function stopRequestedFor(model: string, deps?: Partial<SurfaceDeps>): Promise<boolean> {
  const d = withDeps(deps)
  const now = Date.now()
  if (now - stopCache.at > STOP_TTL_MS) {
    stopCache = { at: now, models: new Set(await d.readSetting<string[]>(STOP_KEY, []).catch((): string[] => [])) }
  }
  return stopCache.models.has(model)
}

/** Clear the request once the run has actually ended, so the next Start is not
 *  stopped by a flag left over from the last one. */
async function clearStopRequest(model: string, d: SurfaceDeps): Promise<void> {
  const asked = await d.readSetting<string[]>(STOP_KEY, []).catch((): string[] => [])
  if (!asked.includes(model)) return
  await d.writeSetting(STOP_KEY, asked.filter((m) => m !== model)).catch(() => {})
  stopCache = { at: 0, models: new Set() }
}

export type ForgetResult = { ok: true; keys: CapabilityKey[]; models: ModelRow[]; report: boolean } | { ok: false; error: string }

export interface ClearResult {
  /** Models whose results were removed. */
  models: string[]
  /** Archived reports deleted. */
  reports: number
  /** Transcript rows deleted. */
  transcripts: number
}

/** WIPE RECORDED RESULTS so a candidate can be tested from nothing.
 *
 *  THIS IS NOT `forgetModel`, and the difference is the whole reason both exist.
 *  Forget throws away what we know a model CAN DO — probe facts, measured once
 *  and true until the id is re-pointed at different weights — and is the release
 *  valve for exactly that. This throws away what a RUN FOUND: the report, its
 *  index entry, the resume ledger and the archived transcripts. An admin who has
 *  just fixed a fixture wants the second one and emphatically not the first,
 *  because re-probing is nine calls they have already paid for.
 *
 *  ALL FOUR, OR IT DOES NOT WORK. Clearing the report but leaving the sweep
 *  status behind is the trap: the model reads as untested and the next Start
 *  resumes into a run that is already complete, returning instantly having
 *  bought nothing. Clearing the report but leaving the index entry behind leaves
 *  the matrix pointing at a report that is gone, which is the one state the
 *  detail route cannot serve.
 *
 *  `model === null` clears every tested candidate. */
export async function clearFitnessResults(model: string | null, deps?: Partial<SurfaceDeps>): Promise<ClearResult> {
  const d = withDeps(deps)
  const index = await d.readSetting<FitnessIndex>(INDEX_KEY, {})
  const targets = model === null ? Object.keys(index) : [storedIdFor(model, canonicalIndex(index, await d.models().catch((): GatewayModel[] => [])))]

  let reports = 0
  for (const id of targets) {
    if (id in index) reports++
    await d.writeSetting(recordKey(id), null)
    // The resume ledger, which is the half everyone forgets.
    await d.clearEvalStatus(id).catch(() => {})
  }
  const rest = Object.fromEntries(Object.entries(index).filter(([id]) => !targets.includes(id)))
  await d.writeSetting(INDEX_KEY, rest)
  const transcripts = await d.clearTranscripts(model === null ? null : targets[0]!).catch(() => 0)
  return { models: targets, reports, transcripts }
}

/** Audit 1.2's release valve, per endpoint:model rather than per id: a model id
 *  re-pointed at different weights has facts about something else, and the
 *  gateway's learned-parameter ratchet has no other way out.
 *
 *  IT FORGETS THE REPORT TOO, and until it did, the button did not appear to
 *  work at all. Talaria records what it knows about a model in TWO places, and
 *  this only ever cleared one of them:
 *
 *    capability facts    `model_capabilities`, per endpoint:model. Cleared.
 *    the archived report `model_fitness_report:<id>` plus its `INDEX_KEY` entry
 *                        — the probe verdicts, the per-slot bands, the
 *                        adversarial rate and the "tested <date>" line. NOT
 *                        cleared, so an admin pressed Forget, the panel
 *                        refetched, and every number they had just been told was
 *                        deleted was still on the screen.
 *
 *  The confirm dialog has always promised "probe results ... are deleted", which
 *  is the correct promise for a valve whose whole purpose is a model id pointed
 *  at new weights: a verdict measured against the old ones is not stale, it is
 *  about a different model. So the fix is to keep the promise rather than narrow
 *  it. `writeSetting(key, null)` is how `evictArchive` already deletes a report;
 *  this uses the same door.
 *
 *  A MISSING REPORT IS NOT AN ERROR. Forgetting a model nobody has swept clears
 *  the facts and reports `report: false` — the valve is idempotent, and failing
 *  because there was nothing to delete would be a worse surface than saying so. */
export async function forgetModel(model: string, deps?: Partial<SurfaceDeps>): Promise<ForgetResult> {
  const d = withDeps(deps)
  const rows = await modelRows(d)
  const row = rows.find((r) => r.id === model)
  if (!row) return { ok: false, error: 'that model is not on the gateway' }
  const keys = keysFor(row)
  for (const key of keys) await d.forget(key)

  // The index entry goes with the record, in that order: an index naming a
  // report that is already gone is the one state the detail route cannot serve.
  // RAW index, and the STORED id: Forget deletes rows, so it has to name them
  // the way they are actually filed. The button is pressed from a canonical
  // row, so the two can differ — and a Forget that deleted neither while
  // reporting success is exactly the bug this function was written to fix.
  const index = await d.readSetting<FitnessIndex>(INDEX_KEY, {})
  const stored = storedIdFor(model, canonicalIndex(index, await d.models().catch((): GatewayModel[] => [])))
  const report = stored in index
  await d.writeSetting(recordKey(stored), null)
  if (report) {
    const { [stored]: _gone, ...rest } = index
    await d.writeSetting(INDEX_KEY, rest)
  }
  return { ok: true, keys, models: await modelRows(d), report }
}

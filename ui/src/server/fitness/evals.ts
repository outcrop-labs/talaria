// TIER 2 OF THE MODEL FITNESS SUITE — harness conformance.
//
// WHAT IT ANSWERS, and it is not "is this a good model": for each harness in
// THIS install, can the candidate model hold that harness's contract, and where
// exactly does it break? A new model release should be a fifteen-minute sweep
// and a swap, not a week of production surprises.
//
// THIS IS A DRIVER, NOT A SUBSYSTEM. Everything it needs already exists:
//   registry.ts   enumerates the harnesses and hands each definition back with
//                 its I and O still paired (`RegisteredHarness.use`)
//   define.ts     `EvalCase` — a fixture input plus a deterministic `check`
//   run.ts        `runHarness`, which already takes `ctx.model` (the pin) and
//                 `ctx.deps.transport` (the seam `run.test.ts` drives it
//                 through). Replaying a fixture against a candidate is those
//                 two fields and nothing else.
// So the whole of tier 2 is: for every fixture, pin the candidate, run it, and
// read the numbers off the row the runner already writes.
//
// THE FIVE NUMBERS, and why `repairRate` is the one that matters:
//   contractRate  the contract held on the FIRST attempt
//   repairRate    the contract held at all, after the repair turn (CUMULATIVE —
//                 see the field comment). A model at 40% first-pass and 95%
//                 after one repair is USABLE; one at 40/45 is not, and until
//                 this file existed nothing in Talaria could tell those two
//                 apart. That distinction is the entire argument for audit 1.4.
//   taskScore     the fixture's own deterministic `check`. No model judging a
//                 model — see `EvalCase` in define.ts for why, and judge.ts for
//                 the one harness that cannot be a string assertion and is
//                 scored by AGREEMENT with a labeled set instead.
//   guardRate     guard findings per run, from the pass the runner already does
//   latency/cost  what the sweep actually spent
//
// THE PREDICATE IS THE RUNNER'S OWN, READ OFF THE ROW IT WRITES.
//   `harness_runs.schema_valid` and the offline fixtures disagreed once, and the
//   production column was the optimistic liar: blurb-writer's schema
//   (`z.record(z.string(), z.string())`) cannot constrain the KEYS, so a reply
//   keyed by tidied-up display names wrote zero blurbs and recorded a perfect
//   contract, while the fixture had rejected that exact reply the whole time.
//   `output.verify` closed that gap by giving the input-relational half of the
//   contract somewhere to live.
//
//   A benchmark that re-derived "did the contract hold" would reopen it: two
//   spellings of the predicate is how the two halves came to disagree in the
//   first place. So this file NEVER decides that question. It captures the
//   `HarnessRunRow` the runner hands to `recordRun` — the literal row production
//   reads — and scores `row.schemaValid`, `row.repairs`, `row.findings` and
//   `row.latencyMs`. If the benched number and the observed number ever diverge
//   again, it is because the model changed, not because the ruler did.
//
//   The fixture's `check` is scored SEPARATELY, as `taskScore`, and the two are
//   allowed to disagree — that is what `optimistic` counts. See its comment for
//   which readings are expected and which one is a bug.
//
// WHAT THE SWEEP DELIBERATELY DOES NOT WRITE: `harness_runs` rows and
// `guard_findings` rows. Both tables are the OBSERVED half of the fitness page,
// shown next to the benched half, and a sweep that filed into them would move
// the number it is being compared against — seventy benched judge runs would
// swamp a week of real ones and make a model's live confabulation rate a
// property of how often an admin pressed Test. The guard pass still RUNS (that
// is where `guardRate` comes from); only the filing is suppressed. Token spend
// is real spend and still reaches `usage_events` through the transports, which
// is why the sweep's caller names itself.
import { getSetting, setSetting } from '../audit'
import { getGuardConfig } from '../guardrails'
import { listActivityHarnesses, type HarnessSource, type RegisteredHarness } from '../harness/registry'
import { runHarness, type HarnessDeps, type HarnessResult, type HarnessRunRow } from '../harness/run'
import { defaultTransport, type Transport, type TransportRequest } from '../harness/transport'
import { estimateTokens } from '../usage'
import type { Capability } from '../harness/capability'
import type { EvalCase, HarnessDefinition } from '../harness/define'

// ── The scoring surface ──────────────────────────────────────────────────────

/** One fixture, replayed once against the candidate. */
export interface EvalCaseScore {
  harness: string
  /** `EvalCase.name`. Unique within a harness; `caseKey` joins the two. */
  case: string

  /** THE CONTRACT HELD — `harness_runs.schema_valid`, taken from the row the
   *  runner wrote rather than recomputed here. See the file header. */
  contractHeld: boolean
  /** It held WITHOUT a repair turn. `repairs === 0 && contractHeld`, which is
   *  exact: the runner's attempt loop breaks the moment the contract holds, so
   *  a zero repair count on a valid run means the first reply was valid. */
  firstPass: boolean
  repairs: number
  /** The model produced a reply the contract could be applied to. False for
   *  every way a run ends without one — the capability floor refused it, the
   *  chain routed nothing, the transport died — and in each of those `error`
   *  carries the runner's own sentence naming which. That sentence is the
   *  drill-down; this flag alone cannot tell a refused floor from a dead
   *  gateway, and re-deriving the floor predicate here to find out would be a
   *  second copy of the rule `run.ts` owns. */
  answered: boolean

  /** THE FIXTURE'S OWN DETERMINISTIC CHECK.
   *  'unscored' when the contract failed: there is no model value to grade, and
   *  counting a contract failure as a task failure would double-charge one
   *  fault. Also 'unscored' for a harness whose `onFailure` is `{ fallback }` —
   *  the runner hands back the DECLARED SAFE VALUE with `schemaValid: false`,
   *  and grading that would award the model task points for a constant its
   *  author wrote. */
  task: 'pass' | 'fail' | 'unscored'
  /** The fixture's one-line reason, verbatim. This is what an admin reads in
   *  the drill-down, which is why `EvalCase.check` is documented to write it
   *  for a human rather than for a developer. */
  taskError: string | null

  /** Guard findings this run is EVIDENCE for — `harness_runs.findings`, which
   *  excludes grounded hits exactly as `recordFindings` does. */
  findings: number
  latencyMs: number
  promptTokens: number
  completionTokens: number
  /** Null when nothing priced the tokens — see `EvalDeps.price`. */
  costUsd: number | null
  /** True when the token counts are a chars/4 estimate because the transport
   *  reported no usage. A cost built on estimated tokens is an estimate. */
  estimated: boolean

  /** The case did not settle inside the bound and the sweep moved on. A hanging
   *  harness must never strand the sweep — see `runEvalSweep`. */
  timedOut: boolean

  /** THE CONTRACT HELD AND THE FIXTURE REJECTED THE VALUE ANYWAY.
   *
   *  Not automatically a bug, and the two readings are worth keeping apart:
   *
   *    EXPECTED where the fixture grades QUALITY the contract deliberately does
   *    not police. The judge agreeing with a label, a title being 3-7 words, a
   *    blurb fitting on one line: blurb-writer's header says in so many words
   *    that length is measured and not enforced, because failing a batch of ten
   *    over one long sentence would cost nine good ones. That is `taskScore`
   *    doing its job.
   *
   *    A BUG where the fixture is asserting something the CALLER depends on —
   *    the keys are the ids that were sent, the elements are tickets from the
   *    transcript, the date is one the write path accepts. Then production is
   *    recording `schema_valid: true` for a value the caller will throw away,
   *    which is audit finding 1.1 with green telemetry over it. The fix is
   *    `output.verify` (define.ts), not a change here.
   *
   *  `HarnessScore.verifies` is the tell: a JSON harness with fixtures and no
   *  `verify` has no way to express the second kind at all. */
  optimistic: boolean

  /** The runner's failure sentence, redacted and bounded by `run.ts`. Null on a
   *  clean run. */
  error: string | null
  /** DRILL-DOWN, kept only for cases that failed something — the actual prompt
   *  and the actual response, which is what makes a red cell trustworthy
   *  instead of merely alarming. A clean case carries neither: seventy passing
   *  transcripts in a settings row is an archive, not telemetry. */
  prompt: string | null
  raw: string | null
}

/** Everything the sweep needs to know about a harness that is not a score.
 *  Split out so `scoreHarnesses` stays pure over recorded cases. */
export interface HarnessMeta {
  id: string
  label: string
  source: HarnessSource
  outputKind: 'text' | 'json'
  requires: Capability[]
  /** Does this harness declare the input-relational half of its contract? See
   *  `EvalCaseScore.optimistic`. */
  verifies: boolean
  /** CAN a repair turn happen here at all? `run.ts` sets `maxRepairs` to 0 for
   *  every text harness — the one repair wording lives in json.ts and ends
   *  "send the corrected JSON value only", which is nonsense to a titler — and
   *  thirteen of the registry's harnesses are text.
   *
   *  Without this flag `repairRate` and `contractRate` are equal on those
   *  thirteen for a structural reason, and `repairYield` would read 0 ("the
   *  repair turn rescued nothing") where the honest answer is "no repair turn
   *  was ever sent". A benchmark that prints a rescue rate for a round-trip
   *  that cannot occur is exactly the kind of confidently wrong number this
   *  whole audit is about. */
  repairable: boolean
}

/** The per-harness column of the fitness matrix. */
export interface HarnessScore extends HarnessMeta {
  cases: number
  /** Cases that reached a verdict — everything except a timeout. */
  scored: number

  /** THE CONTRACT HELD ON THE FIRST ATTEMPT, over all cases. */
  contractRate: number
  /** THE CONTRACT HELD AT ALL, over all cases — CUMULATIVE, so it is always
   *  >= `contractRate` and the pair reads the way the audit states it: 40/95 is
   *  a usable model with a repair path, 40/45 is not. A conditional rate would
   *  print 92 and 8 for those two and bury the decision. */
  repairRate: number
  /** Of the cases that failed first, the share the repair turn RECOVERED. The
   *  conditional number, kept because it is the one that says whether spending
   *  a second round-trip on this model is worth it.
   *
   *  Null when the question does not arise: nothing failed first, or the
   *  harness cannot repair at all (`repairable`). Zero means the repair turn
   *  ran and rescued nothing, which is a different and much worse fact. */
  repairYield: number | null

  /** Fixture checks passed, over the cases that were task-scorable. Null when
   *  none were — a model that never held the contract has no task score, and
   *  printing 0 would blame it twice. */
  taskScore: number | null
  /** Findings per run. */
  guardRate: number
  answeredRate: number

  latencyP50: number
  latencyP95: number
  promptTokens: number
  completionTokens: number
  /** Sum of the priced cases; null when nothing was priced. */
  costUsd: number | null
  estimated: boolean

  timeouts: number
  optimistic: number
}

export type EvalSweepState = 'idle' | 'running' | 'stopped' | 'done' | 'error'

/** THE RESUMABLE, BOUNDED RUN, in `app_settings` — the same shape and the same
 *  lifecycle as `ReindexStatus` in retrieval/migrate.ts, deliberately. Talaria
 *  has one long-run mechanism and this is it; a second one would be a second
 *  set of stuck-state bugs to learn about.
 *
 *  It carries the scored cases as well as the progress, and that is what makes
 *  the run RESUMABLE rather than merely observable: a sweep stopped by an admin
 *  (or interrupted by a deploy) can be restarted and will skip what it already
 *  paid for. Seventy cases of bounded text is a few KB. */
export interface EvalSweepStatus {
  state: EvalSweepState
  /** The candidate this sweep is about. Resuming only ever continues a sweep of
   *  the SAME model — scores from two models in one set would be a matrix cell
   *  that means nothing. */
  model: string | null
  startedAt: string | null
  finishedAt: string | null
  /** Cases finished / cases planned. */
  done: number
  total: number
  /** The harness currently running, for the progress line. */
  harness: string | null
  error: string | null
  cases: EvalCaseScore[]
}

/** The finished (or stopped) sweep, scored. */
export interface EvalSweep {
  model: string
  state: EvalSweepState
  startedAt: string | null
  finishedAt: string | null
  done: number
  total: number
  error: string | null
  harnesses: HarnessScore[]
  cases: EvalCaseScore[]
  /** Registered harnesses that declare NO fixtures. They are invisible to tier
   *  2 — not passing, not failing — and an admin reading a full green matrix
   *  deserves to know which columns were never tested. */
  unfixtured: string[]
  /** Was the guard pass actually on? With `mode: 'off'` every `guardRate` is
   *  zero, and zero-because-off must not read as zero-because-clean. */
  guarded: boolean
}

// ── Injected edges ───────────────────────────────────────────────────────────

export interface EvalDeps {
  harnesses: () => Promise<RegisteredHarness[]>
  /** Passed through to `runHarness` as `ctx.deps`. The sweep adds its own
   *  transport wrapper, `recordRun` and `recordFindings` on top (see the file
   *  header for why the last two are suppressed); everything else — model
   *  resolution, capability facts, the guard config — stays REAL, because the
   *  capability floor refusing a weak model IS a tier-2 result and a fake that
   *  never refuses would be testing the fake. */
  harnessDeps: Partial<HarnessDeps>
  readStatus: () => Promise<EvalSweepStatus>
  writeStatus: (status: EvalSweepStatus) => Promise<void>
  /** Dollars for one call's tokens, or null when this install cannot say.
   *
   *  DEFAULTS TO NULL ON PURPOSE. Talaria prices spend in exactly one place —
   *  the `PRICED` view in usage.ts, over `usage_events`, with a documented
   *  coalesce order across the admin's per-model override, the auto-fetched
   *  public rate and the endpoint default. The sweep's turns land in that table
   *  through the real transports, so the fitness page can join the ledger and
   *  get the same dollars everything else in the product quotes. Re-deriving
   *  that formula here would be a second price that drifts from the first, and
   *  a cost estimate nobody can reconcile with the invoice is worse than none.
   *  The hook is here so a caller that HAS an oracle can supply it. */
  price: (model: string, promptTokens: number, completionTokens: number) => Promise<number | null>
  now: () => number
}

const STATUS_KEY = 'harness_eval_status'

export const IDLE_STATUS: EvalSweepStatus = {
  state: 'idle',
  model: null,
  startedAt: null,
  finishedAt: null,
  done: 0,
  total: 0,
  harness: null,
  error: null,
  cases: [],
}

const REAL_DEPS: EvalDeps = {
  harnesses: listActivityHarnesses,
  harnessDeps: {},
  readStatus: () => getSetting<EvalSweepStatus>(STATUS_KEY, IDLE_STATUS),
  writeStatus: (status) => setSetting(STATUS_KEY, status),
  price: async () => null,
  now: () => Date.now(),
}

/** The live status, for a polling admin panel. */
export const evalSweepStatus = (): Promise<EvalSweepStatus> => REAL_DEPS.readStatus()

export interface EvalOptions {
  /** THE BOUND ON ONE CASE. A harness that hangs — a persona container that
   *  accepts the connection and never answers, an upstream that holds a stream
   *  open forever — must cost the sweep one case, not the whole run. The case
   *  is recorded `timedOut` and the sweep moves on.
   *
   *  It is a race rather than only an `AbortSignal` because the signal is a
   *  REQUEST: a transport that ignores it, or a `render` stuck in author code
   *  before any transport is reached, would leave the sweep waiting on a promise
   *  that never settles. The signal is fired too, so a transport that does honor
   *  it stops burning tokens. */
  caseTimeoutMs?: number
  /** Only these harness ids. Empty/omitted means every registered harness. */
  only?: string[]
  /** Ignore a resumable status and start clean. */
  restart?: boolean
  deps?: Partial<EvalDeps>
}

const DEFAULT_CASE_TIMEOUT_MS = 60_000
/** Bounded for the same reason `HarnessResult.raw` is: a drill-down, not an
 *  archive, and a model that answers with 200KB of prose must not be able to
 *  turn one failed case into a settings row nothing can read. */
const DRILLDOWN_CAP = 4_000

export const caseKey = (harness: string, name: string): string => `${harness}::${name}`

// ── Scoring (pure over recorded cases) ───────────────────────────────────────

/** Nearest-rank percentile. Exact on the small samples a sweep produces — a
 *  harness declares two to five fixtures, and an interpolating percentile over
 *  three numbers invents a latency nothing measured. */
function percentile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0
  const rank = Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1))
  return sorted[rank] ?? 0
}

const rate = (n: number, of: number): number => (of === 0 ? 0 : n / of)

/** Score one harness's cases. Pure, and it takes the METADATA rather than the
 *  registry so that a test can score recorded cases without a registry at all. */
export function scoreHarness(meta: HarnessMeta, cases: EvalCaseScore[]): HarnessScore {
  const total = cases.length
  const scored = cases.filter((c) => !c.timedOut)
  const first = cases.filter((c) => c.firstPass).length
  const held = cases.filter((c) => c.contractHeld).length
  const failedFirst = cases.filter((c) => !c.firstPass).length
  const recovered = cases.filter((c) => c.contractHeld && !c.firstPass).length
  const taskable = cases.filter((c) => c.task !== 'unscored')
  const priced = cases.filter((c) => c.costUsd !== null)
  const latencies = [...scored.map((c) => c.latencyMs)].sort((a, b) => a - b)
  return {
    ...meta,
    cases: total,
    scored: scored.length,
    contractRate: rate(first, total),
    repairRate: rate(held, total),
    repairYield: failedFirst === 0 || !meta.repairable ? null : rate(recovered, failedFirst),
    taskScore: taskable.length === 0 ? null : rate(taskable.filter((c) => c.task === 'pass').length, taskable.length),
    guardRate: rate(
      cases.reduce((n, c) => n + c.findings, 0),
      total,
    ),
    answeredRate: rate(cases.filter((c) => c.answered).length, total),
    latencyP50: percentile(latencies, 0.5),
    latencyP95: percentile(latencies, 0.95),
    promptTokens: cases.reduce((n, c) => n + c.promptTokens, 0),
    completionTokens: cases.reduce((n, c) => n + c.completionTokens, 0),
    costUsd: priced.length === 0 ? null : priced.reduce((n, c) => n + (c.costUsd ?? 0), 0),
    estimated: cases.some((c) => c.estimated),
    timeouts: cases.filter((c) => c.timedOut).length,
    optimistic: cases.filter((c) => c.optimistic).length,
  }
}

/** Group recorded cases by harness and score each. Order follows `metas`, which
 *  is the registry's order — the order Admin shows the harnesses in. */
export function scoreHarnesses(metas: HarnessMeta[], cases: EvalCaseScore[]): HarnessScore[] {
  const out: HarnessScore[] = []
  for (const meta of metas) {
    const mine = cases.filter((c) => c.harness === meta.id)
    if (mine.length === 0) continue
    out.push(scoreHarness(meta, mine))
  }
  return out
}

export const metaOf = (h: RegisteredHarness): HarnessMeta => ({
  id: h.id,
  label: h.label,
  source: h.source,
  outputKind: h.outputKind,
  requires: h.requires,
  verifies: h.use((def) => def.output.verify !== undefined),
  // Mirrors `maxRepairs` in run.ts, which is the only thing that decides
  // whether a repair turn happens: JSON output, and a `repair` count the
  // harness did not zero out.
  repairable: h.use((def) => def.output.kind === 'json' && (def.output.repair ?? 1) > 0),
})

// ── Stopping ─────────────────────────────────────────────────────────────────

/** In-process, exactly like `reindexRunning` in retrieval/migrate.ts: one node
 *  runs the sweep and the Stop button reaches the same node. It is a REQUEST,
 *  honored between cases and by aborting the case in flight, so a stop always
 *  lands on a case boundary and the persisted status is never half a case. */
let sweeping = false
let stopRequested = false

/** Ask the running sweep to stop. Returns whether one was running to ask. */
export function stopEvalSweep(): boolean {
  if (!sweeping) return false
  stopRequested = true
  return true
}

/** Is a sweep running IN THIS PROCESS? A persisted `state: 'running'` with this
 *  false is a sweep a restart interrupted, which is resumable rather than
 *  stuck — see `runEvalSweep`. */
export const evalSweepRunning = (): boolean => sweeping

// ── The driver ───────────────────────────────────────────────────────────────

interface CaseRun {
  row: HarnessRunRow | null
  prompt: string
  promptTokens: number
  completionTokens: number
  estimated: boolean
  threw: string | null
  timedOut: boolean
}

/** The transport the sweep wraps around the real one: it changes nothing about
 *  the call and records the prompt and the token counts, which no other seam
 *  can see. `HarnessResult` carries neither by design — the drill-down needs
 *  the prompt and the cost line needs the tokens, and widening the runner's
 *  result type for a benchmark would put benchmark concerns in the hot path. */
function recordingTransport(base: Transport, into: CaseRun): Transport {
  return async (req: TransportRequest) => {
    into.prompt = req.messages.map((m) => `${m.role}: ${m.content}`).join('\n\n')
    const reply = await base(req)
    if (reply.usage) {
      into.promptTokens += reply.usage.promptTokens
      into.completionTokens += reply.usage.completionTokens
    } else {
      // The same chars/4 fallback the token ledger uses, from the same helper —
      // a second estimator would give the fitness page and the invoice two
      // different token counts for one call.
      into.promptTokens += estimateTokens(into.prompt.length)
      into.completionTokens += estimateTokens(reply.text.length)
      into.estimated = true
    }
    return reply
  }
}

/** Race a promise against a wall clock. The loser is not cancelled — it cannot
 *  be, which is the whole point (see `EvalOptions.caseTimeoutMs`) — so the run
 *  promise is given a `.catch` BEFORE the race: a transport that rejects five
 *  minutes after the sweep moved on must not surface as an unhandled rejection
 *  that takes the process with it. */
async function bounded<T>(work: Promise<T>, ms: number, onTimeout: () => void): Promise<{ done: true; value: T } | { done: false }> {
  let timer: ReturnType<typeof setTimeout> | null = null
  const settled = work.then((value) => ({ done: true, value }) as const)
  const expiry = new Promise<{ done: false }>((resolve) => {
    timer = setTimeout(() => {
      onTimeout()
      resolve({ done: false })
    }, ms)
  })
  try {
    return await Promise.race([settled, expiry])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

const cap = (text: string | null): string | null => (text ? text.slice(0, DRILLDOWN_CAP) : null)

async function runOneCase<I, O>(
  def: HarnessDefinition<I, O>,
  fixture: EvalCase<I, O>,
  model: string,
  deps: EvalDeps,
  timeoutMs: number,
): Promise<EvalCaseScore> {
  const capture: CaseRun = { row: null, prompt: '', promptTokens: 0, completionTokens: 0, estimated: false, threw: null, timedOut: false }
  const controller = new AbortController()

  const harnessDeps: Partial<HarnessDeps> = {
    ...deps.harnessDeps,
    transport: recordingTransport(deps.harnessDeps.transport ?? defaultTransport, capture),
    // The row the runner writes IS the predicate this file scores. Captured
    // here and NOT forwarded to the real `recordRun`: see the file header on why
    // a sweep must not file into the table it is being compared against.
    recordRun: async (row) => {
      capture.row = row
    },
    // Same argument, and stronger — `guard_findings.model` is the live
    // per-model confabulation rate, and seventy benched runs would rewrite it.
    // The guard pass still runs; only the filing is suppressed.
    recordFindings: async () => {},
  }

  const work = runHarness(def, fixture.input, {
    caller: `fitness:${def.id}`,
    model,
    signal: controller.signal,
    deps: harnessDeps,
  })
    .catch((err: unknown): HarnessResult<O> | null => {
      // `onFailure: 'throw'` harnesses (research synthesis, channel-plan) throw
      // out of the runner by declaration. A benchmark that let that escape would
      // let one harness's failure policy end the sweep, which is the opposite of
      // what a sweep is for. The row is already captured — `run.ts` writes it
      // before it throws, precisely so a throwing harness stays visible.
      capture.threw = err instanceof Error ? err.message : String(err)
      return null
    })

  const outcome = await bounded(work, timeoutMs, () => {
    capture.timedOut = true
    controller.abort()
  })

  const row = capture.row
  // The typed result, straight off the race — never read back out of the
  // capture, so the fixture's `check` sees an O rather than an `unknown` this
  // file had to assert its way back to.
  const result: HarnessResult<O> | null = outcome.done ? outcome.value : null
  const contractHeld = row ? row.schemaValid : (result?.schemaValid ?? false)
  const repairs = row ? row.repairs : (result?.repairs ?? 0)
  const firstPass = contractHeld && repairs === 0

  // The fixture grades a MODEL VALUE or nothing. A contract failure has no value
  // to grade, and `onFailure: { fallback }` hands back the harness author's
  // declared constant with `schemaValid: false` over it — grading that would
  // award the model marks for a value it never produced.
  let task: EvalCaseScore['task'] = 'unscored'
  let taskError: string | null = null
  if (contractHeld && result && result.value !== null) {
    try {
      taskError = fixture.check(result.value)
    } catch (err) {
      // A fixture check is author code meeting model output, the same as
      // `clean` and `verify`, and `run.ts` holds those to "a throw is a
      // failure, never an escaped exception". A sweep that died on one badly
      // written assertion would take 22 other harnesses with it.
      taskError = `the fixture check threw on the value: ${err instanceof Error ? err.message : String(err)}`
    }
    task = taskError === null ? 'pass' : 'fail'
  }

  const clean = outcome.done && contractHeld && task !== 'fail'
  const costUsd =
    capture.promptTokens + capture.completionTokens > 0 ? await deps.price(model, capture.promptTokens, capture.completionTokens).catch(() => null) : null

  return {
    harness: def.id,
    case: fixture.name,
    contractHeld,
    firstPass,
    repairs,
    answered: result?.answered ?? false,
    task,
    taskError,
    findings: row?.findings ?? 0,
    latencyMs: row?.latencyMs ?? 0,
    promptTokens: capture.promptTokens,
    completionTokens: capture.completionTokens,
    costUsd,
    estimated: capture.estimated,
    timedOut: capture.timedOut,
    optimistic: contractHeld && task === 'fail',
    error: capture.timedOut ? `the case did not finish inside ${timeoutMs}ms` : (capture.threw ?? row?.error ?? result?.error ?? null),
    prompt: clean ? null : cap(capture.prompt),
    raw: clean ? null : cap(result?.raw ?? null),
  }
}

/** THE SWEEP: every fixture in the registry, replayed against one candidate.
 *
 *  RESUMABLE. The persisted status carries the scored cases, so a sweep an admin
 *  stopped — or a deploy interrupted — restarts where it left off rather than
 *  re-buying seventy calls. Resume is only ever within ONE candidate: a status
 *  for a different model is discarded, because a matrix cell assembled from two
 *  models is a number with no referent.
 *
 *  BOUNDED. Every case races a wall clock (`EvalOptions.caseTimeoutMs`) and a
 *  case that loses is recorded `timedOut` and left behind. No single harness can
 *  strand the sweep, whatever its transport does.
 *
 *  SEQUENTIAL, on purpose. A parallel sweep measures the gateway's queue rather
 *  than the model, so `latencyP50` would stop meaning what the fitness page
 *  says it means; and a self-hosted 14B behind one GPU answers a parallel sweep
 *  with rate-limit errors that would score as contract failures. */
export async function runEvalSweep(model: string, opts: EvalOptions = {}): Promise<EvalSweep> {
  const deps: EvalDeps = { ...REAL_DEPS, ...opts.deps }
  const timeoutMs = opts.caseTimeoutMs ?? DEFAULT_CASE_TIMEOUT_MS

  // One sweep at a time in this process, exactly as `reindexAll` does it. A
  // second concurrent sweep would interleave two candidates' cases into one
  // status row. The caller gets the RUNNING sweep's progress back rather than
  // an error — the second press of a Test button means "show me the run", not
  // "start a second one" — with no harness scores on it, because scoring a
  // half-finished sweep would print a contract rate over the cases that
  // happened to be done.
  if (sweeping) return sweepOf(await deps.readStatus().catch(() => IDLE_STATUS), [], [], false)
  sweeping = true
  stopRequested = false

  const iso = (at: number): string => new Date(at).toISOString()
  try {
    const all = await deps.harnesses()
    const wanted = opts.only?.length ? all.filter((h) => opts.only?.includes(h.id)) : all
    const metas = wanted.map(metaOf)
    const unfixtured = wanted.filter((h) => h.evalNames.length === 0).map((h) => h.id)

    // A persisted status for THIS model is a resume point. A persisted
    // 'running' with `sweeping` false is a sweep a restart interrupted — also
    // resumable, and treating it as stuck instead would leave the feature
    // permanently unusable after one unlucky deploy.
    const prior = await deps.readStatus().catch(() => IDLE_STATUS)
    const resumable = !opts.restart && prior.model === model && prior.state !== 'idle' && prior.state !== 'done'
    const cases: EvalCaseScore[] = resumable ? [...prior.cases] : []
    const already = new Set(cases.map((c) => caseKey(c.harness, c.case)))

    const total = wanted.reduce((n, h) => n + h.evalNames.length, 0)
    const startedAt = resumable ? prior.startedAt : iso(deps.now())

    const write = (state: EvalSweepState, harness: string | null, error: string | null): Promise<void> =>
      deps
        .writeStatus({
          state,
          model,
          startedAt,
          finishedAt: state === 'running' ? null : iso(deps.now()),
          done: cases.length,
          total,
          harness,
          error,
          cases,
        })
        .catch(() => {})

    await write('running', null, null)

    // Read once, and report it: with the guard off every `guardRate` is zero,
    // and zero-because-off must never be read as zero-because-clean.
    const guardConfig = deps.harnessDeps.guardConfig ?? getGuardConfig
    const guarded = await guardConfig()
      .then((c) => c.mode !== 'off')
      .catch(() => false)

    for (const harness of wanted) {
      if (stopRequested) break
      const pending = harness.evalNames.filter((name) => !already.has(caseKey(harness.id, name)))
      if (pending.length === 0) continue
      // Persisted AFTER EVERY CASE, not after every harness. The status is both
      // the progress bar and the resume ledger, and a sweep that checkpointed
      // per harness would re-buy a whole harness's fixtures after a restart —
      // and, on the slowest harnesses, show a progress bar that does not move
      // for minutes.
      await harness.use(<I, O>(def: HarnessDefinition<I, O>) =>
        runHarnessCases(def, pending, model, deps, timeoutMs, () => stopRequested, async (score) => {
          cases.push(score)
          await write('running', harness.id, null)
        }),
      )
    }

    const state: EvalSweepState = stopRequested ? 'stopped' : 'done'
    await write(state, null, null)
    return sweepOf({ state, model, startedAt, finishedAt: iso(deps.now()), done: cases.length, total, harness: null, error: null, cases }, metas, unfixtured, guarded)
  } catch (err) {
    // Same shape as `reindexAll`: the failure lands in the status rather than
    // escaping to a route handler, because the admin who pressed the button is
    // watching this row and not a stack trace.
    const message = err instanceof Error ? err.message : String(err)
    const prior = await deps.readStatus().catch(() => IDLE_STATUS)
    const failed: EvalSweepStatus = { ...prior, state: 'error', model, finishedAt: iso(deps.now()), harness: null, error: message }
    await deps.writeStatus(failed).catch(() => {})
    return sweepOf(failed, [], [], false)
  } finally {
    sweeping = false
    stopRequested = false
  }
}

/** One harness's pending fixtures, in declaration order, stopping between cases
 *  when a stop was asked for. Split out so that `def`'s I and O stay paired
 *  under `use` — the closure remembers the types the registry erased. */
async function runHarnessCases<I, O>(
  def: HarnessDefinition<I, O>,
  pending: string[],
  model: string,
  deps: EvalDeps,
  timeoutMs: number,
  stopped: () => boolean,
  onCase: (score: EvalCaseScore) => Promise<void>,
): Promise<void> {
  for (const fixture of def.evals ?? []) {
    if (stopped()) break
    if (!pending.includes(fixture.name)) continue
    await onCase(await runOneCase(def, fixture, model, deps, timeoutMs))
  }
}

function sweepOf(status: EvalSweepStatus, metas: HarnessMeta[], unfixtured: string[], guarded: boolean): EvalSweep {
  return {
    model: status.model ?? '',
    state: status.state,
    startedAt: status.startedAt,
    finishedAt: status.finishedAt,
    done: status.done,
    total: status.total,
    error: status.error,
    harnesses: scoreHarnesses(metas, status.cases),
    cases: status.cases,
    unfixtured,
    guarded,
  }
}

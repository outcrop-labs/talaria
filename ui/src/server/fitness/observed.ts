// PRODUCTION TELEMETRY AS FITNESS SIGNAL — the half no external benchmark can
// give you, and the half nothing was reading.
//
// Two tables already carry it and both were written for other reasons:
//
//   `harness_runs`     one row per harness exit, from the runner: which model,
//                      which chain step actually won, did the contract hold,
//                      how many repairs, how many findings, how long. This is
//                      the OBSERVED twin of everything `fitness/evals.ts`
//                      benches, measured on real work.
//   `guard_findings`   one row per filed finding, from `recordFindings`. Model
//                      is a column; `guardStats()` already aggregates by check
//                      and `guardCoachingFor` already runs this query shape for
//                      an agent. Aggregating by MODEL is the live confabulation
//                      rate, and nobody was doing it.
//
// THE ALERT THIS EXISTS FOR: a model that benched Ready and is running at a 12%
// repair rate in production. `divergences` is that alert, and the sample gate on
// it is load-bearing — three production runs is not a divergence, it is noise.
//
// ── TWO POPULATIONS. DO NOT ADD THEM. ───────────────────────────────────────
//
// `harness_runs.findings` and `guard_findings` rows are THE SAME EVENTS counted
// twice from different ends: the runner guards a reply, `recordFindings` files a
// row per ungrounded finding, and the runner also stamps the COUNT of those same
// findings onto its `harness_runs` row. Summing them double-counts every harness
// finding in the install.
//
// They are also not the same POPULATION. `guard_findings` is broader — the
// public gateway route, chat and channel replies all file into it and none of
// them writes a `harness_runs` row — so it has no run denominator anywhere in
// the schema. That is why this module returns:
//
//   a RATE   `ObservedHarness.findingsPerRun`, from `harness_runs` alone, over
//            the harness population, which is the only population with a
//            denominator. This is what the fitness verdict compares against.
//   COUNTS   `ObservedModel.guardFindings` / `guardByCheck`, from
//            `guard_findings` alone. Counts, never a rate, because dividing
//            them by harness runs would price gateway traffic against a
//            denominator that never contained it.
//
// Nothing here returns their sum, and `observed.test.ts` asserts that.
//
// BOTH POPULATIONS ARE UNGROUNDED-ONLY, and for one reason stated in one place:
// `recordFindings` drops grounded findings before it files, because a finding
// raised by the model repeating an identifier out of its own input is a fact
// about the INPUT, not about the model. The runner's `findings` count applies
// the same filter. So the two numbers are consistently scoped even though they
// must not be added.
//
// THE SWEEP DOES NOT APPEAR IN EITHER. `fitness/evals.ts` suppresses `recordRun`
// and `recordFindings`, and `fitness/probes.ts` does the same, precisely so that
// pressing Test cannot move the number Test is being compared against.
import { db } from '../db/pg'
import type { HarnessScore } from './evals'

/** Default lookback. Long enough that a weekly-ish harness (the librarian, the
 *  concluder) has a sample, short enough that a model swapped out a month ago
 *  stops dragging its old numbers into today's verdict. */
export const DEFAULT_WINDOW_DAYS = 30

/** Below this, a production/bench gap is sampling noise and reporting it would
 *  train people to ignore the alert. */
export const MIN_OBSERVED_RUNS = 20

/** How far apart tested and observed must be before it is worth a line. */
export const DIVERGENCE_THRESHOLD = 0.1

// ── Rows ─────────────────────────────────────────────────────────────────────

/** One (harness, model) pair as production actually ran it.
 *
 *  The three contract numbers use the SAME definitions `fitness/evals.ts` uses
 *  for the benched half, deliberately and to the letter: `contractRate` is the
 *  contract holding on the first attempt, `repairRate` is it holding at all
 *  (cumulative, so it is always >= `contractRate`), and `repairedShare` is the
 *  conditional-free share of runs that needed a repair and got one. Two
 *  spellings of the predicate is how `harness_runs.schema_valid` and the eval
 *  fixtures came to disagree once already; if the benched and observed numbers
 *  ever diverge again it must be because the model changed, not the ruler. */
export interface ObservedHarness {
  harness: string
  /** Null is a real, recorded outcome: nothing routed for this run. */
  model: string | null
  runs: number
  contractRate: number
  repairRate: number
  /** THE 12% NUMBER. Share of runs that took a repair turn and were saved by
   *  it — the one an admin watches after a swap. */
  repairedShare: number
  /** UNGROUNDED guard findings per run. See the population note in the header:
   *  this is the only findings RATE in the module and the only one the verdict
   *  compares against. */
  findingsPerRun: number
  /** Share of runs that got the widened prompt — how often the capability-gated
   *  superpower actually fired for this model. */
  widenedShare: number
  /** Share of runs that recorded a failure sentence. */
  errorRate: number
  /** Which fallback actually carried this harness, by count. A subsystem
   *  limping along on 'first-routable' for a month is a real finding and this
   *  is where it becomes visible. */
  steps: Array<{ step: string; runs: number }>
  latencyP50: number
  lastRunAt: string | null
}

/** One model's whole production footprint. The two findings figures are kept in
 *  separate fields on purpose — see the header. */
export interface ObservedModel {
  model: string
  harnessRuns: number
  /** From `harness_runs` only, over the harness population. */
  harnessFindingsPerRun: number
  /** From `guard_findings` only. A COUNT — this population has no denominator. */
  guardFindings: number
  guardByCheck: Record<string, number>
  /** The confabulation subset of `guardByCheck`: the checks that are claims
   *  about the model inventing something rather than leaking something. */
  confabulation: number
}

/** The checks that mean "this model made something up", as opposed to
 *  `secret_leak` / `pii_leak`, which mean "this model repeated something it
 *  should not have". Both matter and they are different failures, so the
 *  fitness page names them separately. Ids are `RULES` ids from guardrails.ts;
 *  `observed.test.ts` locks them against that registry. */
export const CONFABULATION_CHECKS: readonly string[] = ['zero_tool_claim', 'ungrounded_ref', 'fabricated_outage']

// ── Injected edges ───────────────────────────────────────────────────────────

/** Grouped `harness_runs`, one row per (harness, model). Shaped as the SQL
 *  returns it so a test can hand over rows without a database. */
export interface HarnessRunGroup {
  harness: string
  model: string | null
  runs: number
  firstPass: number
  held: number
  repaired: number
  findings: number
  widened: number
  errors: number
  latencyP50: number
  lastRunAt: string | null
}

export interface ChainStepGroup {
  harness: string
  model: string | null
  step: string | null
  runs: number
}

export interface GuardFindingGroup {
  model: string
  check: string
  n: number
}

export interface ObservedDeps {
  harnessRuns: (sinceDays: number) => Promise<HarnessRunGroup[]>
  chainSteps: (sinceDays: number) => Promise<ChainStepGroup[]>
  guardFindings: (sinceDays: number) => Promise<GuardFindingGroup[]>
}

const REAL_DEPS: ObservedDeps = {
  harnessRuns: async (sinceDays) => {
    const sql = await db()
    return (await sql`
      select
        harness,
        model,
        count(*)::int                                                        as "runs",
        sum(case when schema_valid and repairs = 0 then 1 else 0 end)::int   as "firstPass",
        sum(case when schema_valid then 1 else 0 end)::int                   as "held",
        sum(case when schema_valid and repairs > 0 then 1 else 0 end)::int   as "repaired",
        sum(findings)::int                                                   as "findings",
        sum(case when widened then 1 else 0 end)::int                        as "widened",
        sum(case when error is not null then 1 else 0 end)::int              as "errors",
        coalesce(percentile_disc(0.5) within group (order by latency_ms), 0)::int as "latencyP50",
        max(created_at)                                                      as "lastRunAt"
      from harness_runs
      where created_at > now() - (${sinceDays} || ' days')::interval
      group by harness, model
    `) as unknown as HarnessRunGroup[]
  },
  chainSteps: async (sinceDays) => {
    const sql = await db()
    return (await sql`
      select harness, model, chain_step as "step", count(*)::int as "runs"
      from harness_runs
      where created_at > now() - (${sinceDays} || ' days')::interval
      group by harness, model, chain_step
    `) as unknown as ChainStepGroup[]
  },
  guardFindings: async (sinceDays) => {
    const sql = await db()
    return (await sql`
      select model, check_type as "check", count(*)::int as "n"
      from guard_findings
      where model is not null and created_at > now() - (${sinceDays} || ' days')::interval
      group by model, check_type
    `) as unknown as GuardFindingGroup[]
  },
}

export interface ObservedOptions {
  sinceDays?: number
  /** Only this candidate. Omitted returns every model that ran, which is what
   *  the matrix wants. */
  model?: string
  deps?: Partial<ObservedDeps>
}

const rate = (n: number, of: number): number => (of === 0 ? 0 : n / of)

const iso = (at: string | Date | null): string | null => (at === null ? null : at instanceof Date ? at.toISOString() : at)

// ── Reads ────────────────────────────────────────────────────────────────────

/** Production, per harness per model. Empty on a fresh install, and empty is
 *  the correct answer there rather than a reason to hide the panel. */
export async function observedHarnesses(opts: ObservedOptions = {}): Promise<ObservedHarness[]> {
  const deps: ObservedDeps = { ...REAL_DEPS, ...opts.deps }
  const days = opts.sinceDays ?? DEFAULT_WINDOW_DAYS
  // Advisory data behind a verdict that must still render. A telemetry query
  // that throws must not take the fitness page with it — the page falls back to
  // "no production data", which is exactly what an install with no rows shows.
  const [groups, steps] = await Promise.all([deps.harnessRuns(days).catch(() => []), deps.chainSteps(days).catch(() => [])])
  const wanted = opts.model ? groups.filter((g) => g.model === opts.model) : groups
  return wanted.map((g) => ({
    harness: g.harness,
    model: g.model,
    runs: g.runs,
    contractRate: rate(g.firstPass, g.runs),
    repairRate: rate(g.held, g.runs),
    repairedShare: rate(g.repaired, g.runs),
    findingsPerRun: rate(g.findings, g.runs),
    widenedShare: rate(g.widened, g.runs),
    errorRate: rate(g.errors, g.runs),
    steps: steps
      .filter((s) => s.harness === g.harness && s.model === g.model)
      .map((s) => ({ step: s.step ?? 'none', runs: s.runs }))
      .sort((a, b) => b.runs - a.runs),
    latencyP50: g.latencyP50,
    lastRunAt: iso(g.lastRunAt),
  }))
}

/** Production, per model, across both tables — with the two findings figures
 *  kept apart. There is deliberately no field summing them; see the header. */
export async function observedModels(opts: ObservedOptions = {}): Promise<ObservedModel[]> {
  const deps: ObservedDeps = { ...REAL_DEPS, ...opts.deps }
  const days = opts.sinceDays ?? DEFAULT_WINDOW_DAYS
  const [groups, findings] = await Promise.all([deps.harnessRuns(days).catch(() => []), deps.guardFindings(days).catch(() => [])])

  const models = new Set<string>()
  for (const g of groups) if (g.model !== null) models.add(g.model)
  for (const f of findings) models.add(f.model)

  const out: ObservedModel[] = []
  for (const model of models) {
    if (opts.model && model !== opts.model) continue
    const mine = groups.filter((g) => g.model === model)
    const runs = mine.reduce((n, g) => n + g.runs, 0)
    const guardByCheck: Record<string, number> = {}
    let guardFindings = 0
    let confabulation = 0
    for (const f of findings.filter((f) => f.model === model)) {
      guardByCheck[f.check] = (guardByCheck[f.check] ?? 0) + f.n
      guardFindings += f.n
      if (CONFABULATION_CHECKS.includes(f.check)) confabulation += f.n
    }
    out.push({
      model,
      harnessRuns: runs,
      harnessFindingsPerRun: rate(
        mine.reduce((n, g) => n + g.findings, 0),
        runs,
      ),
      guardFindings,
      guardByCheck,
      confabulation,
    })
  }
  return out.sort((a, b) => b.harnessRuns - a.harnessRuns || a.model.localeCompare(b.model))
}

// ── The baseline the verdict compares against ────────────────────────────────

/** Findings per run per HARNESS, across every model that ran it — which is what
 *  "the current production baseline for that role" means: what this install
 *  puts up with today, not what the candidate does.
 *
 *  ACROSS MODELS ON PURPOSE. Baselining a candidate against its own production
 *  history would make a model that has been confabulating for a month its own
 *  reference and score it clean. Weighted by runs so one harness that ran twice
 *  cannot set the bar for one that ran ten thousand times.
 *
 *  Absent harnesses are absent, not zero: `score.ts` treats a missing entry as a
 *  zero bar and SAYS SO in the reason, which is a different sentence from "we
 *  measured zero". */
export function guardBaseline(rows: ObservedHarness[]): Record<string, number> {
  const runs: Record<string, number> = {}
  const findings: Record<string, number> = {}
  for (const r of rows) {
    runs[r.harness] = (runs[r.harness] ?? 0) + r.runs
    findings[r.harness] = (findings[r.harness] ?? 0) + r.findingsPerRun * r.runs
  }
  const out: Record<string, number> = {}
  for (const [harness, n] of Object.entries(runs)) out[harness] = rate(findings[harness] ?? 0, n)
  return out
}

// ── Tested vs observed ───────────────────────────────────────────────────────

export type DivergenceMetric = 'contract' | 'repair' | 'guard'

/** One benched number that production does not agree with. */
export interface Divergence {
  harness: string
  model: string
  metric: DivergenceMetric
  tested: number
  observed: number
  /** observed - tested. Negative on `contract` means production is WORSE; on
   *  `repair` and `guard`, positive means production is worse. `worse` says so
   *  without anyone having to remember which. */
  delta: number
  worse: boolean
  observedRuns: number
  /** One sentence for the admin, naming the direction and the sample. */
  note: string
}

export interface DivergenceOptions {
  minRuns?: number
  threshold?: number
}

/** Where the bench and production disagree, for one candidate.
 *
 *  BOTH DIRECTIONS ARE REPORTED, with `worse` set, because both are worth
 *  knowing and only one of them is an alert: production worse than the bench is
 *  the "benched Ready, running at a 12% repair rate" case; production BETTER
 *  than the bench usually means the fixtures are harder than the real traffic,
 *  which is a fact about the fixtures and belongs in front of whoever wrote
 *  them.
 *
 *  `guard` compares like with like: `HarnessScore.guardRate` and
 *  `ObservedHarness.findingsPerRun` are both ungrounded-only findings per run,
 *  from the same guard pass and the same `RULES` registry. It never touches
 *  `guard_findings`, whose population has no run denominator — see the header. */
export function divergences(
  model: string,
  tested: HarnessScore[],
  observed: ObservedHarness[],
  opts: DivergenceOptions = {},
): Divergence[] {
  const minRuns = opts.minRuns ?? MIN_OBSERVED_RUNS
  const threshold = opts.threshold ?? DIVERGENCE_THRESHOLD
  const out: Divergence[] = []

  for (const score of tested) {
    const live = observed.find((o) => o.harness === score.id && o.model === model)
    if (!live || live.runs < minRuns) continue

    const push = (
      metric: DivergenceMetric,
      testedValue: number,
      observedValue: number,
      worseWhenHigher: boolean,
      what: string,
      // Findings per run is not a percentage and printing it as one ("12%
      // findings") would be a number with the wrong unit on an admin page.
      show: (n: number) => string,
    ): void => {
      const delta = observedValue - testedValue
      if (Math.abs(delta) < threshold) return
      const worse = worseWhenHigher ? delta > 0 : delta < 0
      out.push({
        harness: score.id,
        model,
        metric,
        tested: testedValue,
        observed: observedValue,
        delta,
        worse,
        observedRuns: live.runs,
        note: `${score.label} benched ${what} at ${show(testedValue)} and production is running at ${show(observedValue)} over ${live.runs} run(s) — ${worse ? 'worse than the bench' : 'better than the bench'}.`,
      })
    }
    const asPct = (n: number): string => `${Math.round(n * 100)}%`
    const asPerRun = (n: number): string => `${n.toFixed(2)}/run`

    push('contract', score.contractRate, live.contractRate, false, 'first-try contract', asPct)
    // The share of runs the repair turn had to rescue. Derived from the pair
    // rather than carried, and exactly: `contractRate` counts cases that held
    // with zero repairs and `repairRate` counts cases that held at all, so the
    // difference is the cases that needed a repair and got one — the same
    // quantity `ObservedHarness.repairedShare` counts directly.
    push('repair', Math.max(0, score.repairRate - score.contractRate), live.repairedShare, true, 'repair-carried runs', asPct)
    push('guard', score.guardRate, live.findingsPerRun, true, 'guard findings', asPerRun)
  }

  return out.sort((a, b) => Number(b.worse) - Number(a.worse) || Math.abs(b.delta) - Math.abs(a.delta))
}

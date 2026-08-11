// PRICE AGAINST PERFORMANCE, weighed on the work this deployment actually does.
//
// THE QUESTION THIS ANSWERS. The matrix says whether a model can hold a slot.
// It does not say what holding it costs, and it treats the harness a fleet runs
// four thousand times a day exactly like the one the librarian runs on Sundays.
// So an admin looking at two green columns has no way to tell that one model is
// forty times the price of the other for a difference that lands on 2% of their
// traffic. That is the decision this module is for.
//
// ── THE TWO AXES, AND WHY THEY ARE WEIGHED THE SAME WAY ──────────────────────
//
// COST is not $/MTok. A sticker price cannot be compared across models because
// the harnesses differ enormously in shape: the concluder reads a whole session
// and writes four lines (prompt-heavy), the blurb writer reads a sentence and
// writes a paragraph (completion-heavy). Priced per token they look alike;
// priced per day they do not. So cost here is **what a day of your measured
// workload would cost on this model** — the runs production actually did, times
// the tokens a sweep actually measured, times this model's actual price.
//
// PERFORMANCE is not an average of contract rates. score.ts's header refuses to
// mint one and it is right: four harnesses' rates averaged into a scalar is a
// number with no referent. What DOES have a referent is coverage — **the share
// of your daily runs this model is Ready for**, and the shares that fall to
// Workable, Not-a-fit and never-measured beside it. Nothing is imputed: a
// harness nobody tested lands in `untested`, never in the numerator.
//
// Both axes are weighted by the same runs-per-day vector, which is what makes
// the pair a comparison rather than two unrelated charts.
//
// ── WHAT THIS MODULE WILL NOT DO ─────────────────────────────────────────────
//
// It will not fill a hole with an assumption. A model nothing prices has
// `usdPerDay: null` and is drawn off the cost axis rather than at zero. A
// harness no sweep has measured tokens for is EXCLUDED from the cost sum and
// COUNTED in `unmeasured`, so the figure is reported as a floor instead of
// quietly understating the bill. A fresh install with no production rows gets
// the uniform basis — one run of everything — and the payload says so, because
// "we assumed your traffic is flat" is a materially different claim from "this
// is your traffic".
import { BAND_ORDER, harnessBands, type FitnessBand, type FitnessReport, type SlotBinding, type SlotKind } from './score'
import type { RegisteredHarness } from '../harness/registry'
import type { ObservedHarness } from './observed'
import type { FitnessIndex, FitnessIndexEntry, ModelPrice, TokenBudget } from './surface'

// ── The per-harness half of an index entry ───────────────────────────────────

/** What one run measured about one harness, small enough to sit in the matrix
 *  index rather than in the multi-hundred-kilobyte report. */
export interface HarnessSummary {
  band: FitnessBand
  cases: number
  prompt: number
  completion: number
}

/** Collapse a report and its sweep scores into that half.
 *
 *  ONE DEFINITION, TWO CALLERS: `indexEntryOf` writes it when a run finishes,
 *  and `valueView` derives it on the fly for reports archived before the field
 *  existed. Two spellings of "per-case tokens" is how the archived entry and
 *  the live one would come to disagree about what a model costs. */
export function harnessSummary(
  report: FitnessReport,
  scores: ReadonlyArray<{ id: string; cases: number; promptTokens: number; completionTokens: number }>,
): Record<string, HarnessSummary> {
  const swept = new Map(scores.map((s) => [s.id, s]))
  const out: Record<string, HarnessSummary> = {}
  for (const [id, band] of Object.entries(harnessBands(report))) {
    const s = swept.get(id)
    // `cases: 0` — the report judged this harness but the sweep never called
    // it. Recorded as zero rather than divided by zero, and read downstream as
    // "no tokens of this model's own", never as "this harness is free".
    const cases = s?.cases ?? 0
    out[id] = {
      band,
      cases,
      prompt: cases > 0 ? Math.round((s?.promptTokens ?? 0) / cases) : 0,
      completion: cases > 0 ? Math.round((s?.completionTokens ?? 0) / cases) : 0,
    }
  }
  return out
}

// ── The workload ─────────────────────────────────────────────────────────────

/** WHERE THE RUNS-PER-DAY VECTOR CAME FROM, which changes how much of the rest
 *  of this payload an admin should believe.
 *
 *  `observed` — grouped `harness_runs` over the telemetry window, summed across
 *  every model that served each harness. This is the real shape of the day.
 *
 *  `uniform` — nothing has run yet, so every fixtured harness is counted once.
 *  It makes the page useful on day one and it is NOT a measurement; the UI says
 *  which basis it drew. */
export type WorkloadBasis = 'observed' | 'uniform'

export interface Workload {
  basis: WorkloadBasis
  windowDays: number
  /** harness id → runs per day. */
  runs: Record<string, number>
  /** Sum of the above. Zero only on the impossible empty registry. */
  perDay: number
  /** Runs per day landing on harnesses that declare no eval fixtures. Real work
   *  that no run can ever score — reported separately so the untested share can
   *  say WHY it is untested rather than reading as an admin's oversight. */
  unfixturedPerDay: number
  /** Distinct harnesses with any volume. */
  harnesses: number
}

export function workloadFrom(observed: readonly ObservedHarness[], registry: readonly RegisteredHarness[], windowDays: number): Workload {
  const days = Math.max(windowDays, 1)
  const known = new Set(registry.map((h) => h.id))
  const runs: Record<string, number> = {}

  for (const row of observed) {
    // A harness that has since been deleted from the registry still has rows in
    // `harness_runs`. Counting it would put volume in the denominator that no
    // model can ever be scored on, which reads as every model getting worse.
    if (!known.has(row.harness)) continue
    runs[row.harness] = (runs[row.harness] ?? 0) + row.runs / days
  }

  const observedTotal = Object.values(runs).reduce((a, b) => a + b, 0)
  if (observedTotal <= 0) {
    // THE UNIFORM BASIS deliberately covers only fixtured harnesses. On the
    // observed basis an unfixtured harness with real traffic is a real hole and
    // is reported as one; inventing traffic for it here would manufacture the
    // hole instead of finding it.
    const fixtured = registry.filter((h) => h.evalNames.length > 0)
    for (const h of fixtured) runs[h.id] = 1
    return { basis: 'uniform', windowDays: days, runs, perDay: fixtured.length, unfixturedPerDay: 0, harnesses: fixtured.length }
  }

  const unfixtured = new Set(registry.filter((h) => h.evalNames.length === 0).map((h) => h.id))
  const unfixturedPerDay = Object.entries(runs).reduce((n, [id, v]) => (unfixtured.has(id) ? n + v : n), 0)
  return {
    basis: 'observed',
    windowDays: days,
    runs,
    perDay: observedTotal,
    unfixturedPerDay,
    harnesses: Object.keys(runs).length,
  }
}

// ── What one model's last run says about each harness ────────────────────────

/** Per-case tokens for one (model, harness), and WHOSE verbosity they measure.
 *
 *  `model` — this model's own sweep measured them. The only basis that prices a
 *  terse model and a chatty one differently, which is most of what separates
 *  their bills.
 *
 *  `shared` — the harness's entry in the global token budget, measured from
 *  whichever candidate last swept it. Right about the prompt (the fixtures are
 *  fixed) and only approximately right about the completion. Reports written
 *  before this view existed have no per-model tokens and land here.
 *
 *  `none` — nothing has ever measured this harness. Excluded from the sum. */
export type TokenBasis = 'model' | 'shared' | 'none'

export interface HarnessTokens {
  prompt: number
  completion: number
  basis: TokenBasis
}

export function tokensFor(harness: string, entry: FitnessIndexEntry | undefined, budget: TokenBudget): HarnessTokens {
  // ZERO IS NOT A MEASUREMENT anywhere in this chain. A run that failed every
  // case before a token moved records 0/0, and reading that as "this harness is
  // free" would print a confident $0.00 for a model nobody has priced anything
  // about. `recordBudget` no longer writes such an entry; this also declines to
  // read the ones already on disk.
  const own = entry?.harnesses?.[harness]
  if (own && own.cases > 0 && own.prompt + own.completion > 0) return { prompt: own.prompt, completion: own.completion, basis: 'model' }
  const shared = budget[harness]
  if (shared && shared.prompt + shared.completion > 0) return { prompt: shared.prompt, completion: shared.completion, basis: 'shared' }
  return { prompt: 0, completion: 0, basis: 'none' }
}

/** THE BAND THIS MODEL EARNED ON THIS HARNESS.
 *
 *  Prefers `entry.harnesses`, which `indexEntryOf` collapses out of the report
 *  at write time and is the only source that can speak for an UNBOUND harness
 *  (no slot, therefore no cell).
 *
 *  Falls back to the cells for entries written before that field existed: worst
 *  band across the slots this harness is bound to, the same reduction
 *  `harnessBands` does. An old entry therefore reports its bound harnesses
 *  correctly and its unbound ones as `untested` — understating a model rather
 *  than flattering it, which is the right direction to be wrong in. */
export function bandFor(harness: string, entry: FitnessIndexEntry | undefined, slotsOf: ReadonlyMap<string, string[]>): FitnessBand {
  if (!entry) return 'untested'
  const own = entry.harnesses?.[harness]
  if (own) return own.band
  let worst: FitnessBand | null = null
  for (const key of slotsOf.get(harness) ?? []) {
    const band = entry.cells[key]?.band
    if (band === undefined) continue
    if (worst === null || BAND_ORDER[band] < BAND_ORDER[worst]) worst = band
  }
  return worst ?? 'untested'
}

/** harness id → the slot keys it is bound to. Inverted once per read. */
export const slotsByHarness = (bindings: readonly SlotBinding[]): Map<string, string[]> => {
  const out = new Map<string, string[]>()
  for (const b of bindings) {
    const key = `${b.slot.kind}:${b.slot.id}`
    for (const h of b.harnesses) out.set(h.id, [...(out.get(h.id) ?? []), key])
  }
  return out
}

// ── One model's row ──────────────────────────────────────────────────────────

export interface ModelValue {
  model: string
  /** When the run behind these bands finished. Null means never tested, and the
   *  row is still emitted — "you are paying this much for a model nobody has
   *  measured" is one of the more useful things this table can say. */
  at: string | null
  price: ModelPrice | null
  /** Your measured day, priced on this model. Null when nothing prices it. */
  usdPerDay: number | null
  /** THE PRICE-TO-PERFORMANCE NUMBER, and the one with an actual referent:
   *  dollars per run this model is trusted to do. A cheap model that is Ready
   *  for a tenth of your day is not cheap. Null when the cost is null or the
   *  ready share is zero — a division nobody can act on. */
  usdPerReadyRun: number | null
  /** Share of daily runs by band. Sums to 1 across the workload. */
  shares: Record<FitnessBand, number>
  readyShare: number
  /** Ready or Workable — what the model can carry, with a repair turn allowed. */
  usableShare: number
  /** Share of daily runs whose tokens something has measured. Below 1 the cost
   *  is a FLOOR, and the UI says so rather than printing a confident total. */
  costCoverage: number
  /** Whose verbosity `usdPerDay` was computed from. `model` only when every
   *  priced harness used this model's own sweep. */
  tokenBasis: TokenBasis
}

const EMPTY_SHARES: Record<FitnessBand, number> = { ready: 0, workable: 0, unfit: 0, untested: 0, unbound: 0 }

export function valueOf(args: {
  model: string
  entry: FitnessIndexEntry | undefined
  price: ModelPrice | null
  workload: Workload
  budget: TokenBudget
  slotsOf: ReadonlyMap<string, string[]>
}): ModelValue {
  const { model, entry, price, workload, budget, slotsOf } = args
  const shares: Record<FitnessBand, number> = { ...EMPTY_SHARES }
  let usd = 0
  let pricedRuns = 0
  let sawShared = false
  let sawOwn = false

  for (const [harness, perDay] of Object.entries(workload.runs)) {
    if (perDay <= 0) continue
    shares[bandFor(harness, entry, slotsOf)] += perDay
    const tokens = tokensFor(harness, entry, budget)
    if (tokens.basis === 'none') continue
    if (tokens.basis === 'model') sawOwn = true
    else sawShared = true
    pricedRuns += perDay
    if (price) usd += (perDay * (tokens.prompt * price.in + tokens.completion * price.out)) / 1e6
  }

  const total = workload.perDay
  if (total > 0) for (const band of Object.keys(shares) as FitnessBand[]) shares[band] /= total

  // A PRICE WITH NOTHING TO PRICE IS NOT $0. The gemma run that failed every
  // case has a perfectly good $/MTok and not one measured token, and reporting
  // that as "$0 a day" would put the most expensive-looking model on the page
  // at the cheap end of the chart. No measurement, no figure.
  const usdPerDay = price === null || pricedRuns === 0 ? null : usd
  const readyRuns = shares.ready * total
  return {
    model,
    at: entry?.at ?? null,
    price,
    usdPerDay,
    usdPerReadyRun: usdPerDay === null || readyRuns <= 0 ? null : usdPerDay / readyRuns,
    shares,
    readyShare: shares.ready,
    usableShare: shares.ready + shares.workable,
    costCoverage: total > 0 ? pricedRuns / total : 0,
    tokenBasis: sawOwn && !sawShared ? 'model' : sawOwn || sawShared ? 'shared' : 'none',
  }
}

// ── One slot's row ───────────────────────────────────────────────────────────

/** A slot, its share of the day, and what each model that can hold it costs to
 *  run IT — not the whole workload. An admin choosing the Research model wants
 *  the Research bill, and a model's whole-day cost is dominated by whatever
 *  harness happens to run most. */
export interface SlotValue {
  key: string
  label: string
  kind: SlotKind
  live: boolean
  /** Runs per day across the harnesses bound to this slot. A harness bound to
   *  two slots counts in both: this is per-slot demand, not a partition, and
   *  summing the column would double-count on purpose-shared harnesses. */
  perDay: number
  harnesses: number
  /** Every model that reaches Workable or better, cheapest first with Ready
   *  ahead of Workable. Empty means nothing tested can hold this slot. */
  candidates: Array<{ model: string; band: FitnessBand; usdPerDay: number | null }>
  /** The cheapest Ready candidate — the actual recommendation. Null when none
   *  is Ready, which is a finding rather than a gap. */
  best: string | null
}

// ── The read ─────────────────────────────────────────────────────────────────

export interface ValueDeps {
  observed: () => Promise<ObservedHarness[]>
  harnesses: () => Promise<RegisteredHarness[]>
  bindings: (harnesses: RegisteredHarness[]) => Promise<SlotBinding[]>
  index: () => Promise<FitnessIndex>
  budget: () => Promise<TokenBudget>
  price: (model: string) => Promise<ModelPrice | null>
  /** The archived report, read ONLY to backfill an index entry written before
   *  it carried its per-harness half — see `backfill`. Null (or a throw) leaves
   *  that model on the cells and the shared budget, which is what it would have
   *  had anyway. */
  record: (model: string) => Promise<{ report: FitnessReport; harnesses: HarnessScoreLike[] } | null>
  windowDays: number
}

/** The two fields this module reads off a `HarnessScore`, named rather than
 *  imported: `evals.ts` owns thirty of them and none of the rest matter here. */
export interface HarnessScoreLike {
  id: string
  cases: number
  promptTokens: number
  completionTokens: number
}

/** FILL IN WHAT AN OLDER RUN ALREADY MEASURED.
 *
 *  Every number on this page needs per-harness bands and per-harness tokens.
 *  New runs write them into the index; reports archived before that field
 *  existed have them only in the full record — which is real, paid-for
 *  measurement, and stranding it behind "re-test this model" would be asking an
 *  admin to buy a sweep twice.
 *
 *  BOUNDED AND SELF-LIMITING: at most one read per index entry MISSING the
 *  field, at most `KEEP_MODELS` of those, and each one stops needing it the
 *  next time that model is tested. Deliberately not written back — a GET that
 *  rewrites the archive is a surprise, and the read it saves is one an admin
 *  pays only while old entries survive eviction. */
async function backfill(index: FitnessIndex, read: ValueDeps['record']): Promise<FitnessIndex> {
  const stale = Object.keys(index).filter((m) => index[m]?.harnesses === undefined)
  if (stale.length === 0) return index
  const filled = { ...index }
  await Promise.all(
    stale.map(async (model) => {
      const entry = filled[model]
      // `entry.model`, not the KEY: the index is keyed by the id the catalog
      // offers and the report is filed under the id the run used. See
      // `storedIdFor` in surface.ts.
      const record = await read(entry?.model ?? model).catch(() => null)
      if (!record || !entry) return
      filled[model] = { ...entry, harnesses: harnessSummary(record.report, record.harnesses) }
    }),
  )
  return filled
}

export interface ValueView {
  workload: Workload
  models: ModelValue[]
  slots: SlotValue[]
  /** Harnesses carrying volume that no sweep has ever measured tokens for.
   *  Non-empty means every `usdPerDay` on the page is a floor. */
  unmeasured: string[]
  /** True when at least one model priced. False turns the cost axis off rather
   *  than drawing every model at zero. */
  priced: boolean
}

export async function valueView(deps: ValueDeps): Promise<ValueView> {
  const [observed, registry, archived, budget] = await Promise.all([
    deps.observed().catch((): ObservedHarness[] => []),
    deps.harnesses(),
    deps.index(),
    deps.budget().catch((): TokenBudget => ({})),
  ])
  const index = await backfill(archived, deps.record)
  const bindings = await deps.bindings(registry).catch((): SlotBinding[] => [])
  const slotsOf = slotsByHarness(bindings)
  const workload = workloadFrom(observed, registry, deps.windowDays)

  // ONLY MODELS WITH A REPORT. A row for every id on the gateway would be four
  // hundred rows of "untested, 0% ready", which is true and is noise; the
  // matrix above is where an untested model is chosen to be tested.
  const models = Object.keys(index).sort()
  const prices = await Promise.all(models.map((m) => deps.price(m).catch((): ModelPrice | null => null)))
  const rows = models.map((model, i) =>
    valueOf({ model, entry: index[model], price: prices[i] ?? null, workload, budget, slotsOf }),
  )

  const slots: SlotValue[] = bindings.map((b) => {
    const key = `${b.slot.kind}:${b.slot.id}`
    const ids = b.harnesses.map((h) => h.id)
    const perDay = ids.reduce((n, id) => n + (workload.runs[id] ?? 0), 0)
    const candidates = rows
      .flatMap((row) => {
        const band = index[row.model]?.cells[key]?.band ?? 'untested'
        if (band !== 'ready' && band !== 'workable') return []
        // This slot's own bill: the harnesses bound to it, and nothing else.
        // Null unless something here was actually measured, for the same reason
        // the whole-workload figure is — see `valueOf`.
        const price = row.price
        let usd: number | null = null
        if (price) {
          for (const id of ids) {
            const t = tokensFor(id, index[row.model], budget)
            if (t.basis === 'none') continue
            usd = (usd ?? 0) + ((workload.runs[id] ?? 0) * (t.prompt * price.in + t.completion * price.out)) / 1e6
          }
        }
        return [{ model: row.model, band, usdPerDay: usd }]
      })
      .sort((a, b2) => {
        if (a.band !== b2.band) return BAND_ORDER[b2.band] - BAND_ORDER[a.band]
        // An unpriced candidate sorts last within its band: it may well be the
        // cheapest, and a page that ranked it first would be guessing.
        if (a.usdPerDay === null) return b2.usdPerDay === null ? a.model.localeCompare(b2.model) : 1
        if (b2.usdPerDay === null) return -1
        return a.usdPerDay - b2.usdPerDay
      })
    return {
      key,
      label: b.slot.label,
      kind: b.slot.kind,
      live: b.slot.live,
      perDay,
      harnesses: ids.length,
      candidates,
      best: candidates.find((c) => c.band === 'ready')?.model ?? null,
    }
  })

  const measured = new Set([...Object.keys(budget), ...Object.values(index).flatMap((e) => Object.keys(e.harnesses ?? {}))])
  const unmeasured = Object.entries(workload.runs)
    .filter(([id, runs]) => runs > 0 && !measured.has(id))
    .map(([id]) => id)
    .sort()

  return { workload, models: rows, slots, unmeasured, priced: rows.some((r) => r.usdPerDay !== null) }
}

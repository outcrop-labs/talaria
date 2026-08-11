// The admin surface's own rules, tested where they are decidable: the band
// vocabulary, the three-valued capability tag, the estimate sentence in front
// of a button that spends money, and the assignment warning.
//
// The scoring itself is not retested here — score.test.ts owns every band
// boundary. What these lock is the PRESENTATION, and specifically the two ways
// this page could lie: an untested cell reading as a pass, and an unmeasured
// capability reading as a no.
import { describe, expect, it } from 'vitest'
import { BAND_ORDER } from '@/server/fitness/score'
import type { AdversarialBand } from '@/server/fitness/adversarial'
import type { EvalCaseScore } from '@/server/fitness/evals'
import {
  BAND_META,
  BAND_SEVERITY,
  BAND_TEXT,
  CAPABILITY_WORDS,
  TAG_TONE,
  assignmentNotice,
  bandOf,
  DEFAULT_CONCURRENCY,
  caseCategory,
  worthRetrying,
  centsPerRun,
  costCaveat,
  estimateSentence,
  reasonOf,
  rowSummary,
  tagTitle,
  usd,
  usdRate,
  valueVersion,
  visibleTags,
  workloadSentence,
  type CapabilityView,
  type FitnessBand,
  type FitnessIndexEntry,
  type ModelRow,
  type ModelValue,
  type RunEstimate,
  type SlotView,
  type Workload,
} from './fitness'

const slot = (key: string, kind: 'role' | 'agent' = 'role'): SlotView => ({
  kind,
  id: key.split(':')[1] ?? key,
  key,
  label: key,
  hint: '',
  requires: [],
  live: true,
  taskFloor: 0.8,
})

const entry = (cells: Record<string, { band: FitnessBand; reason: string | null }>): FitnessIndexEntry => ({
  model: 'candidate',
  at: '2026-08-06T00:00:00.000Z',
  tiers: ['evals'],
  guarded: true,
  cells,
  safety: null,
  probesWrote: 0,
  speed: null,
  costUsd: null,
  calls: 0,
  partial: false,
})

const view = (over: Partial<CapabilityView> = {}): CapabilityView => ({
  cap: 'search',
  state: 'unknown',
  source: null,
  via: null,
  detail: null,
  score: null,
  at: null,
  ...over,
})

const row = (caps: CapabilityView[], over: Partial<ModelRow> = {}): ModelRow => ({
  id: 'candidate',
  qualified: false,
  endpoints: ['main'],
  pooled: false,
  capabilities: caps,
  ...over,
})

describe('band vocabulary', () => {
  it('never gives an untested or unbound band a tone that reads as a pass', () => {
    // The whole feature fails if grey reads green: an admin swaps a model in on
    // the strength of a cell nobody filled and finds out in production.
    expect(BAND_META.untested.tone).toBe('neutral')
    expect(BAND_META.unbound.tone).toBe('neutral')
    expect(BAND_META.ready.tone).toBe('success')
    expect(BAND_META.unfit.tone).toBe('danger')
  })

  it('says in words that an untested slot is not a pass', () => {
    expect(BAND_META.untested.blurb).toMatch(/not a pass/i)
    expect(BAND_META.unbound.blurb).toMatch(/not a pass/i)
  })

  it('orders bands exactly as score.ts does — the client copy cannot drift', () => {
    // `BAND_SEVERITY` is a literal copy of `score.ts`'s `BAND_ORDER` (see the
    // note on it: importing the server value would pull the harness registry
    // into the browser bundle). This is what stops the copy rotting: a band
    // reordered on the server and not here would have the page summarize a row
    // by one ranking while the verdict was decided by another.
    expect(BAND_SEVERITY).toEqual(BAND_ORDER)
  })

  it('has a colour and a word for every band, including the tier-3 subset', () => {
    // `AdversarialBand` is `Extract<FitnessBand, …>`, so tier 3's verdict lands
    // in `BAND_META` by construction. This is the assertion that fails if
    // anyone re-spells it — which is what it used to be, rendering as raw
    // `not-a-fit` text beside four chips that said "Not a fit".
    const tier3: AdversarialBand[] = ['ready', 'workable', 'unfit']
    for (const band of tier3) expect(BAND_META[band].label.length).toBeGreaterThan(0)
    for (const band of Object.keys(BAND_META) as FitnessBand[]) expect(BAND_TEXT[band]).toMatch(/^text-/)
  })

  it('defaults every cell of an unrun model to untested', () => {
    expect(bandOf(undefined, 'role:utility')).toBe('untested')
    expect(reasonOf(undefined, 'role:utility')).toBeNull()
    // Not merely absent from the map — a model tested for OTHER slots must
    // still read untested on the ones its run never covered.
    expect(bandOf(entry({ 'role:utility': { band: 'ready', reason: null } }), 'role:code-heavy')).toBe('untested')
  })
})

describe('rowSummary', () => {
  const slots = [slot('role:a'), slot('role:b'), slot('role:c')]

  it('counts every band and reports the worst', () => {
    const s = rowSummary(entry({ 'role:a': { band: 'ready', reason: null }, 'role:b': { band: 'unfit', reason: 'x' } }), slots)
    expect(s.counts).toEqual({ ready: 1, workable: 0, unfit: 1, untested: 1, unbound: 0 })
    expect(s.band).toBe('unfit')
  })

  it('ranks untested below workable, so an unmeasured row cannot summarize as the healthier one', () => {
    const measured = rowSummary(entry(Object.fromEntries(slots.map((s) => [s.key, { band: 'workable' as const, reason: null }]))), slots)
    const unmeasured = rowSummary(undefined, slots)
    expect(measured.band).toBe('workable')
    expect(unmeasured.band).toBe('untested')
  })
})

describe('capability tags', () => {
  it('gives unknown a neutral tone — unknown is not false', () => {
    // capability.ts's cardinal rule, carried into the UI: Talaria has to keep
    // working on a model nobody has benchmarked, and a fresh self-host has
    // benchmarked nothing.
    expect(TAG_TONE.unknown).toBe('neutral')
    expect(TAG_TONE.no).toBe('danger')
    expect(TAG_TONE.yes).toBe('success')
  })

  it('writes three different sentences for the three states', () => {
    expect(tagTitle(view({ state: 'unknown' }))).toMatch(/Unknown is not a no/i)
    expect(tagTitle(view({ state: 'yes', source: 'probe', at: 'now', score: 1 }))).toMatch(/can search the web/i)
    expect(tagTitle(view({ state: 'no', source: 'probe', at: 'now' }))).toMatch(/cannot search the web/i)
  })

  it('names its source, so a declared claim is never mistaken for a measurement', () => {
    expect(tagTitle(view({ state: 'yes', source: 'declared', at: 'now' }))).toMatch(/Declared/i)
    expect(tagTitle(view({ state: 'yes', source: 'learned', at: 'now' }))).toMatch(/rejected/i)
    expect(tagTitle(view({ state: 'yes', source: 'probe', at: 'now' }))).toMatch(/probe/i)
  })

  it('hides unknown tags and reports when nothing at all is measured', () => {
    const nothing = visibleTags(row([view(), view({ cap: 'json' })]))
    expect(nothing.anyMeasured).toBe(false)
    expect(nothing.measured).toHaveLength(0)

    const some = visibleTags(row([view({ state: 'no', source: 'probe', at: 'now' }), view({ cap: 'json' })]))
    expect(some.anyMeasured).toBe(true)
    expect(some.measured.map((c) => c.cap)).toEqual(['search'])
  })

  it('has plain words for every capability the tag can carry', () => {
    for (const [cap, words] of Object.entries(CAPABILITY_WORDS)) {
      expect(words.short.length, cap).toBeGreaterThan(0)
      // The plain form completes "this model cannot ___", which is the sentence
      // an admin reads when their assignment is a bad fit.
      expect(words.plain.length, cap).toBeGreaterThan(0)
    }
  })
})

describe('assignmentNotice', () => {
  it('says nothing when the slot is fine', () => {
    expect(assignmentNotice({ entry: entry({ 'role:utility': { band: 'ready', reason: null } }), slotKey: 'role:utility' })).toBeNull()
    expect(assignmentNotice({ entry: entry({ 'role:utility': { band: 'workable', reason: 'slow' } }), slotKey: 'role:utility' })).toBeNull()
  })

  it('says nothing for an untested slot — silence beats a warning nobody earned', () => {
    expect(assignmentNotice({ entry: undefined, slotKey: 'role:utility' })).toBeNull()
  })

  it('prefers the capability sentence, which names what the model cannot do', () => {
    const notice = assignmentNotice({
      entry: entry({ 'role:research-recon': { band: 'unfit', reason: 'research:search held its contract on 20%' } }),
      slotKey: 'role:research-recon',
      capabilityNote: 'Research · Recon needs a model that can search the web, and this one is recorded as unable to.',
    })
    expect(notice?.text).toMatch(/search the web/)
  })

  it('carries the run’s own reason and tells the admin the assignment still stands', () => {
    const notice = assignmentNotice({
      entry: entry({ 'agent:judge': { band: 'unfit', reason: 'judge held its output contract on 61% of 12 fixtures.' } }),
      slotKey: 'agent:judge',
    })
    expect(notice?.band).toBe('unfit')
    expect(notice?.text).toContain('61%')
    // A sentence, not a validation error — the admin may know something the
    // probe does not (audit 1.6).
    expect(notice?.text).toMatch(/still assign it/i)
  })
})

describe('the estimate an admin decides on', () => {
  const est = (over: Partial<RunEstimate> = {}): RunEstimate => ({
    model: 'candidate',
    adversaryModel: null,
    tiers: [],
    calls: 91,
    usd: 0.14,
    priced: true,
    unmeasuredHarnesses: 0,
    fixtures: 68,
    ...over,
  })

  it('leads with the call count, which does not depend on a price catalog', () => {
    expect(estimateSentence(est({ priced: false, usd: null }))).toMatch(/^91 calls/)
    expect(estimateSentence(est({ priced: false, usd: null }))).toMatch(/no dollar figure/i)
  })

  it('says "at least" when some harness has never been measured', () => {
    expect(estimateSentence(est({ unmeasuredHarnesses: 3 }))).toContain('at least')
    expect(estimateSentence(est())).toContain('about')
  })

  it('shows no total when only part of the run could be priced', () => {
    expect(estimateSentence(est({ usd: null }))).toMatch(/no total/i)
  })

  it('never rounds a real spend down to zero', () => {
    // "$0.00" for a four-cent run is the rounding that makes an admin distrust
    // every other number on the page.
    expect(usd(0.004)).toBe('<$0.01')
    expect(usd(0)).toBe('$0')
    expect(usd(null)).toBe('unpriced')
    expect(usd(1.2345)).toBe('$1.23')
  })
})

describe('price against performance, as an admin reads it', () => {
  const workload = (over: Partial<Workload> = {}): Workload => ({
    basis: 'observed',
    windowDays: 30,
    runs: { ticket: 20, brief: 1 },
    perDay: 21,
    unfixturedPerDay: 0,
    harnesses: 2,
    ...over,
  })

  const value = (over: Partial<ModelValue> = {}): ModelValue => ({
    model: 'm',
    at: '2026-08-01T00:00:00.000Z',
    price: { in: 1, out: 4 },
    usdPerDay: 0.04,
    usdPerReadyRun: 0.002,
    shares: { ready: 1, workable: 0, unfit: 0, untested: 0, unbound: 0 },
    readyShare: 1,
    usableShare: 1,
    costCoverage: 1,
    tokenBasis: 'model',
    ...over,
  })

  it('says the uniform basis is an assumption, not your traffic', () => {
    // The single misreading that would cost real money: taking "one run of
    // everything" for a measurement of what the fleet does.
    const s = workloadSentence(workload({ basis: 'uniform', perDay: 26, harnesses: 26 }))
    expect(s).toMatch(/no production runs recorded/i)
    expect(s).toMatch(/equally/i)
  })

  it('names the window and the volume when the basis is real', () => {
    const s = workloadSentence(workload())
    expect(s).toContain('last 30 days')
    expect(s).toContain('21.0 harness runs a day')
  })

  it('reports traffic no test can speak for', () => {
    expect(workloadSentence(workload({ unfixturedPerDay: 3 }))).toMatch(/no fixtures/i)
  })

  it('calls a partial cost a floor rather than printing a confident total', () => {
    expect(costCaveat(value({ costCoverage: 0.7 }), 0)).toMatch(/floor/i)
    expect(costCaveat(value(), 0)).toBeNull()
  })

  it("warns when a model is priced on another model's verbosity", () => {
    expect(costCaveat(value({ tokenBasis: 'shared' }), 0)).toMatch(/another model/i)
  })

  it('says plainly when nothing prices the model', () => {
    expect(costCaveat(value({ usdPerDay: null }), 0)).toMatch(/nothing on this install prices/i)
  })

  it('never rounds a recurring sub-cent bill to zero', () => {
    // $0.004/day is $1.46/year. `usd` would print "<$0.01" and hide it.
    expect(usdRate(0.004, 'day')).toBe('$0.0040/day')
    expect(usdRate(0.4, 'day')).toBe('$0.400/day')
    expect(usdRate(12.5, 'day')).toBe('$12.50/day')
    expect(usdRate(null, 'day')).toBe('unpriced')
  })

  it('shows a per-run cost in cents, and no figure at all when there is none', () => {
    expect(centsPerRun(0.002)).toBe('0.200¢')
    expect(centsPerRun(0.05)).toBe('5.00¢')
    expect(centsPerRun(null)).toBe('—')
  })
})

describe('when the cost tab is allowed to go stale', () => {
  const archived = (model: string, at: string): FitnessIndexEntry => ({ ...entry({}), model, at })

  it('changes when a run lands, which is the only time those numbers move', () => {
    // THE BUG. A run an admin started and then waited out — the normal case —
    // updated the matrix, which polls, and left cost and value showing the
    // numbers from before it, with nothing on screen saying so. Only a POST
    // from the panel invalidated that query.
    const before = valueVersion({ a: archived('a', 'monday') })
    const retested = valueVersion({ a: archived('a', 'friday') })
    const added = valueVersion({ a: archived('a', 'monday'), b: archived('b', 'friday') })

    expect(retested).not.toBe(before)
    expect(added).not.toBe(before)
    expect(valueVersion({})).not.toBe(before)
  })

  it('does not change while a sweep is merely in progress', () => {
    // The other half: this must not become a poll. Nothing about a running
    // sweep touches the archive, so the signature holds still until it lands.
    const index = { a: archived('a', 'monday'), b: archived('b', 'friday') }

    expect(valueVersion(index)).toBe(valueVersion({ ...index }))
    // Key order is not information — two identical archives must sign the same.
    expect(valueVersion({ b: index.b, a: index.a })).toBe(valueVersion(index))
  })
})

// ── Case categories ──────────────────────────────────────────────────────────

describe('caseCategory', () => {
  it('reads the family out of a namespaced harness id', () => {
    expect(caseCategory('muse:draft')).toEqual({ id: 'muse', label: 'Muse' })
    expect(caseCategory('workbench:heavy')).toEqual({ id: 'workbench', label: 'Workbench' })
    expect(caseCategory('research-search')).toEqual({ id: 'research', label: 'Research' })
    expect(caseCategory('inbox-command')).toEqual({ id: 'inbox', label: 'Inbox' })
  })

  it('keeps a whole-id family whole rather than splitting it at the hyphen', () => {
    // 'work-session' is not the 'work' family, and 'blurb-writer' is not 'blurb'.
    expect(caseCategory('work-session')).toEqual({ id: 'agents-at-work', label: 'Agents at work' })
    expect(caseCategory('blurb-writer').label).toBe('Naming')
  })

  it('gives harnesses that share a LABEL one tab, not four tabs with one name', () => {
    // Four harnesses map to "Knowledge". Keyed by family that was four separate
    // tabs all captioned KNOWLEDGE, which is the same word four times rather
    // than a grouping.
    const ids = ['distiller', 'summarizer', 'concluder', 'librarian'].map((h) => caseCategory(h).id)
    expect(new Set(ids).size).toBe(1)
    expect(caseCategory('titler').id).toBe(caseCategory('blurb-writer').id)
  })

  it('gives an unmapped harness its own id as a label instead of a bucket', () => {
    // THE POINT OF DERIVING RATHER THAN LISTING. A harness added next quarter
    // gets its own tab; it does not silently disappear into "Other", which is
    // how a hand-kept table stops covering the thing it is for.
    expect(caseCategory('invoice-reconciler')).toEqual({ id: 'invoice', label: 'invoice' })
  })
})

// ── The one value this module is allowed to copy ─────────────────────────────

describe('worthRetrying', () => {
  const c = (over: Partial<EvalCaseScore>): EvalCaseScore =>
    ({ skipped: null, gap: null, contractHeld: true, task: 'pass', ...over }) as EvalCaseScore

  it('keeps a clean pass and re-opens everything else', () => {
    expect(worthRetrying(c({}))).toBe(false)
    expect(worthRetrying(c({ task: 'fail' }))).toBe(true)
    expect(worthRetrying(c({ contractHeld: false, task: 'unscored' }))).toBe(true)
    // The rate-limited case: unmeasured, and the single best reason to press the
    // button at all.
    expect(worthRetrying(c({ skipped: 'rate limits on every attempt' }))).toBe(true)
    // A gap is re-asked because the usual reason to retry is that somebody just
    // fixed the harness that reported it.
    expect(worthRetrying(c({ gap: 'the fixture never gave it the id', task: 'unscored' }))).toBe(true)
  })

  it('agrees with the server predicate case for case', async () => {
    // A COPY THAT DRIFTS IS WORSE THAN NO BUTTON: the count on the button and
    // the set the sweep actually re-runs would diverge silently, and nobody
    // could tell which was right.
    const server = await import('@/server/fitness/evals')
    for (const over of [
      {},
      { task: 'fail' as const },
      { contractHeld: false },
      { skipped: 'busy' },
      { gap: 'ours' },
      { task: 'unscored' as const },
    ]) {
      expect(worthRetrying(c(over)), JSON.stringify(over)).toBe(server.worthRetrying(c(over)))
    }
  })
})

describe('DEFAULT_CONCURRENCY', () => {
  it('matches the server constant it stands in for', async () => {
    // WHY IT IS A COPY AT ALL. `fitness.ts` is runtime-dependency-free on
    // purpose (see its header): a VALUE import from `@/server/fitness/evals`
    // pulls the sweep driver, the database and the harness runner into the
    // browser bundle, and the Models route stops loading. It happened.
    //
    // A test may import the server module; the browser may not. So the constant
    // is copied and this is what stops the copy from rotting.
    const server = await import('@/server/fitness/evals')
    expect(DEFAULT_CONCURRENCY).toBe(server.DEFAULT_CONCURRENCY)
  })
})


// ── Supplied capabilities ────────────────────────────────────────────────────

describe('a capability the DEPLOYMENT supplies', () => {
  it('gets its own tone — neither the green of yes nor the red of no', () => {
    // Calling it 'yes' claims a blind model sees; calling it 'no' refuses a
    // deployment that can genuinely do the job. It is a third fact.
    expect(TAG_TONE.supplied).not.toBe(TAG_TONE.yes)
    expect(TAG_TONE.supplied).not.toBe(TAG_TONE.no)
    expect(TAG_TONE.supplied).not.toBe(TAG_TONE.unknown)
  })

  it('names the supplier, because "supplied" alone is unverifiable', () => {
    // The supplier is the thing that might be switched off tomorrow, at which
    // point every model leaning on it silently loses the capability.
    const t = tagTitle(view({ cap: 'search', state: 'supplied', via: { server: 'searxng', tool: 'web_search' } }))
    expect(t).toContain('searxng.web_search')
    expect(t).toContain('cannot')
  })

  it('still reads as a capability the install HAS', () => {
    const t = tagTitle(view({ cap: 'vision', state: 'supplied', via: { server: 'talaria', tool: 'describe_image' } }))
    expect(t).toContain('this deployment can')
  })
})

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
import {
  BAND_META,
  BAND_SEVERITY,
  BAND_TEXT,
  CAPABILITY_WORDS,
  TAG_TONE,
  assignmentNotice,
  bandOf,
  estimateSentence,
  reasonOf,
  rowSummary,
  tagTitle,
  usd,
  visibleTags,
  type CapabilityView,
  type FitnessBand,
  type FitnessIndexEntry,
  type ModelRow,
  type RunEstimate,
  type SlotView,
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
  costUsd: null,
  calls: 0,
  partial: false,
})

const view = (over: Partial<CapabilityView> = {}): CapabilityView => ({
  cap: 'search',
  state: 'unknown',
  source: null,
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

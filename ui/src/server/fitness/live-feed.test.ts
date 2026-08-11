import { describe, expect, it } from 'vitest'
import { clearLiveFeed, liveFeedFor, noteLive, startLiveFeed } from './live-feed'
import type { EvalLogLine } from './surface'
import { probeLine, type ProbeResult } from './probes'
import { provocationLine } from './adversarial'
import type { ProvocationScore } from './adversarial'

const line = (over: Partial<EvalLogLine> = {}): EvalLogLine => ({
  harness: 'probes',
  case: 'tools',
  verdict: 'pass',
  ms: 10,
  tokens: 0,
  calls: 0,
  up: null,
  note: null,
  ...over,
})

describe('the tier feed', () => {
  it('keeps a model’s lines in completion order', () => {
    startLiveFeed('m')
    noteLive('m', line({ case: 'json' }))
    noteLive('m', line({ case: 'tools' }))
    expect(liveFeedFor('m').map((l) => l.case)).toEqual(['json', 'tools'])
  })

  it('does not leak one model’s run into another’s console', () => {
    startLiveFeed('a')
    startLiveFeed('b')
    noteLive('a', line({ case: 'json' }))
    expect(liveFeedFor('b')).toEqual([])
  })

  it('starts a tier over rather than replaying the last one', () => {
    // `startLiveFeed` is called when a TIER begins, not when a run does — so
    // adversarial does not open with tier 1's nine probe lines still on screen.
    startLiveFeed('m')
    noteLive('m', line({ case: 'json' }))
    startLiveFeed('m')
    expect(liveFeedFor('m')).toEqual([])
  })

  it('forgets a finished run, so its lines cannot appear above the next one', () => {
    startLiveFeed('m')
    noteLive('m', line())
    clearLiveFeed('m')
    expect(liveFeedFor('m')).toEqual([])
  })

  it('is bounded, so a long adversarial run cannot grow without limit', () => {
    startLiveFeed('m')
    for (let i = 0; i < 500; i++) noteLive('m', line({ case: `p-${i}` }))
    const feed = liveFeedFor('m')
    expect(feed.length).toBeLessThanOrEqual(200)
    // NEWEST KEPT, because a log reads downward and the end is where a watcher
    // is looking.
    expect(feed.at(-1)?.case).toBe('p-499')
  })

  it('never throws into the run it is reporting on', () => {
    // Telemetry on the hot path of a paid run. A feed that can fail the sweep is
    // worse than no feed at all.
    expect(() => noteLive('never-started', line())).not.toThrow()
    expect(liveFeedFor('never-started').length).toBe(1)
  })
})

// ── What a probe and a provocation LOOK like in the console ──────────────────
//
// The mapping is where the judgement is. Both tiers have outcomes that are
// neither a pass nor a failure, and painting those green is how a watcher comes
// away believing the model demonstrated something it did not.

describe('probeLine', () => {
  const r = (outcome: ProbeResult['outcome']): ProbeResult => ({ id: 'tools' as ProbeResult['id'], label: 'Tool calling', outcome })

  it('scores a measured probe by its verdict', () => {
    expect(probeLine(r({ kind: 'scored', trials: [], verdict: { value: true, score: 1, detail: 'called it' } }), 12)).toMatchObject({ harness: 'probes', verdict: 'pass', ms: 12 })
    expect(probeLine(r({ kind: 'scored', trials: [], verdict: { value: false, score: 0, detail: 'never called it' } }), 12).verdict).toBe('fail')
  })

  it('paints an ALREADY-MEASURED probe as a skip, not a pass', () => {
    // No call was made, so nothing was measured on THIS run. Green would tell a
    // watcher the model just demonstrated something it was never asked to do.
    const line = probeLine(r({ kind: 'known', at: '2026-08-01T00:00:00.000Z', trials: [], verdict: { value: true, score: 1, detail: 'from an earlier run' } }), 0)
    expect(line.verdict).toBe('skip')
    expect(line.note).toContain('no call made')
  })

  it('paints a probe that could not be measured as a skip, and one that broke as an error', () => {
    expect(probeLine(r({ kind: 'skipped', reason: 'no vision advertised', trials: [] }), 0)).toMatchObject({ verdict: 'skip', note: 'no vision advertised' })
    expect(probeLine(r({ kind: 'errored', reason: 'the probe did not finish inside 60000ms', trials: [] }), 0).verdict).toBe('error')
  })
})

describe('provocationLine', () => {
  const p = (over: Partial<ProvocationScore>): ProvocationScore => ({ id: 'invent-a-link', target: 'ungrounded_ref', origin: 'seed', from: null, answered: true, silent: false, elicited: false, ...over }) as ProvocationScore

  it('calls an elicited provocation a FAILURE — the model did the thing', () => {
    expect(provocationLine(p({ elicited: true }), 30)).toMatchObject({ harness: 'adversarial', verdict: 'fail' })
    expect(provocationLine(p({ elicited: true }), 30).note).toContain('elicited')
  })

  it('calls a resisted provocation a pass', () => {
    expect(provocationLine(p({}), 30).verdict).toBe('pass')
  })

  it('does NOT let silence read as safety', () => {
    // A model that stonewalls every prompt produced no bad output and also
    // demonstrated nothing. `silent` is counted as resisted on the report and
    // reported separately for exactly this reason; the console has to agree.
    const line = provocationLine(p({ silent: true }), 30)
    expect(line.verdict).toBe('skip')
    expect(line.note).toContain('silence cannot read as safety')
  })

  it('calls a transport failure an error rather than resistance', () => {
    expect(provocationLine(p({ answered: false }), 30)).toMatchObject({ verdict: 'error' })
  })
})

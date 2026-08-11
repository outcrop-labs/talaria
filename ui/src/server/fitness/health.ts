// HARNESS HEALTH — which of OUR fixtures are broken, read across every model.
//
// THE QUESTION THIS ANSWERS, and nothing else on the page can. A red cell tells
// an admin a model failed a fixture. It cannot tell them whether the fixture is
// right. Those are different questions with different owners, and for a month
// the only way to tell them apart was for somebody to notice the same assertion
// failing on a second model and go and read it — which is how every fixture
// rewritten this round was found, one at a time, by hand.
//
// A FIXTURE THAT FAILS ON EVERY MODEL IS OURS. Not certainly — a genuinely hard
// task can defeat a shortlist — but it is the strongest signal available without
// reading transcripts, and it is cheap: the archive already holds every case of
// every run. The evidence for it is on the record already:
//
//     work-session · hands a finished ticket to review    failed on gemma AND deepseek
//     work-session · ends with the status line            failed on gemma AND deepseek
//     workbench · runs the tests before it calls it done   gapped on gemma AND deepseek
//
// All three were the turn budget cutting the session off mid-work. Nothing on
// the fitness page said so; it took a hand-written database query.
//
// WHAT IT DELIBERATELY DOES NOT DO: judge. It counts, groups the reasons
// verbatim, and says how many models agreed. The reading — "this is a hard task"
// versus "this assertion is wrong" — is a human's, and the panel gives them the
// sentences to make it with.
import type { EvalBand } from '../harness/define'
import type { EvalCaseScore } from './evals'

/** One archived run, reduced to what this file needs. */
export interface HealthInput {
  model: string
  cases: readonly EvalCaseScore[]
}

/** How suspicious a fixture is, in the only three readings the counts support. */
export type Suspicion =
  /** Every model that ran it came back wrong. Read the fixture first. */
  | 'ours'
  /** More than half, but not all. Could be a hard task, could be a fixture that
   *  only the strongest model satisfies — worth a look either way. */
  | 'shared'
  /** One model of several. That is what a fitness suite is FOR. */
  | 'model'
  /** Fewer than two models have run it, so nothing can be concluded. Said out
   *  loud rather than defaulting to 'model', because "no evidence" and "the
   *  model's fault" are the two readings this whole file exists to separate. */
  | 'unknown'

export interface FixtureHealth {
  harness: string
  case: string
  band: EvalBand
  /** Models whose archived run recorded a VERDICT on this fixture — a skip is
   *  not a verdict and is counted separately. */
  tested: number
  failed: number
  /** OUR gap, already: the fixture said it could not fairly ask its question. */
  gapped: number
  timedOut: number
  /** The provider never let us ask, or the candidate could not be tested. */
  unmeasured: number
  suspicion: Suspicion
  /** Distinct failure sentences, commonest first, with who saw each. The
   *  sentences are the point: two models failing for the SAME stated reason is a
   *  much stronger signal than two models failing. */
  reasons: Array<{ reason: string; models: string[] }>
}

/** A verdict was reached — the model answered and the fixture judged it. */
const verdictReached = (c: EvalCaseScore): boolean => c.skipped === null
const wrong = (c: EvalCaseScore): boolean => c.skipped === null && (!c.contractHeld || c.task === 'fail' || c.timedOut || c.error !== null)

/** Every fixture in the archive, with what happened to it across models.
 *
 *  PURE over the records it is given, so the panel and a test see the same
 *  arithmetic. Sorted worst first: the fixtures every model failed, then the
 *  ones most models failed, then the rest — which is the order somebody
 *  debugging their own harness wants to read. */
export function harnessHealth(runs: readonly HealthInput[]): FixtureHealth[] {
  const by = new Map<string, FixtureHealth & { reasonMap: Map<string, string[]> }>()

  for (const run of runs) {
    for (const c of run.cases) {
      const key = `${c.harness}::${c.case}`
      const at =
        by.get(key) ??
        ({
          harness: c.harness,
          case: c.case,
          band: c.band,
          tested: 0,
          failed: 0,
          gapped: 0,
          timedOut: 0,
          unmeasured: 0,
          suspicion: 'unknown' as Suspicion,
          reasons: [],
          reasonMap: new Map<string, string[]>(),
        } satisfies FixtureHealth & { reasonMap: Map<string, string[]> })
      by.set(key, at)

      if (!verdictReached(c)) {
        at.unmeasured++
        continue
      }
      at.tested++
      if (c.gap !== null) at.gapped++
      if (c.timedOut) at.timedOut++
      if (wrong(c) || c.gap !== null) {
        at.failed++
        // The fixture's own sentence, or the runner's. Grouped verbatim — a
        // paraphrase here would destroy the signal, which is two models coming
        // back with the SAME words.
        const reason = (c.gap ?? c.taskError ?? c.error ?? 'no reason recorded').slice(0, 240)
        at.reasonMap.set(reason, [...(at.reasonMap.get(reason) ?? []), run.model])
      }
    }
  }

  const out = [...by.values()].map((f) => {
    const { reasonMap, ...rest } = f
    const reasons = [...reasonMap].map(([reason, models]) => ({ reason, models })).sort((a, b) => b.models.length - a.models.length)
    // A GAP IS OURS BY CONSTRUCTION, whatever the counts say: the fixture itself
    // reported that it could not fairly ask its question. One is enough.
    const suspicion: Suspicion =
      rest.gapped > 0
        ? 'ours'
        : rest.tested < 2
          ? 'unknown'
          : rest.failed === rest.tested
            ? 'ours'
            : rest.failed * 2 > rest.tested
              ? 'shared'
              : 'model'
    return { ...rest, reasons, suspicion }
  })

  const rank: Record<Suspicion, number> = { ours: 0, shared: 1, model: 2, unknown: 3 }
  return out
    .filter((f) => f.failed > 0 || f.unmeasured > 0)
    .sort((a, b) => rank[a.suspicion] - rank[b.suspicion] || b.failed - a.failed || a.harness.localeCompare(b.harness))
}

export interface HealthSummary {
  /** Models whose archives were read. Two is the minimum for any conclusion. */
  models: string[]
  fixtures: FixtureHealth[]
  /** Fixtures every model that ran them got wrong. THE NUMBER: it is a count of
   *  our own bugs, and it should be worked to zero rather than explained. */
  ours: number
  shared: number
}

export const summarize = (runs: readonly HealthInput[]): HealthSummary => {
  const fixtures = harnessHealth(runs)
  return {
    models: runs.map((r) => r.model),
    fixtures,
    ours: fixtures.filter((f) => f.suspicion === 'ours').length,
    shared: fixtures.filter((f) => f.suspicion === 'shared').length,
  }
}

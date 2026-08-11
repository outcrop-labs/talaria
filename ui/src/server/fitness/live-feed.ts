// THE LIVE CONSOLE, FOR THE TIERS THAT ARE NOT THE SWEEP.
//
// WHAT WAS MISSING. The fitness terminal is fed from `liveLog(sweep.cases)`, so
// it showed tier 2 and nothing else. Start a run with probes and adversarial
// selected and the console sits blank for the first minutes, prints two hundred
// fixtures, then goes blank again while tier 3 runs — for a watcher, the run
// appears to hang twice at exactly the moments it is doing the most interesting
// work. Probes and provocations are units of work that pass or fail like any
// fixture, and there was no reason beyond plumbing that they were invisible.
//
// WHY IN MEMORY, AND WHY THAT IS THE RIGHT CALL. This is a LIVE feed: the only
// consumer is a panel polling every three seconds while a run is in flight, and
// once the run lands the archived record is the better thing to read (it has the
// full probe report and the full adversarial report, not a one-line summary).
// So it follows `inFlight` in evals.ts exactly — a module-level map, cleared
// when the tier starts, lost on restart, and nothing downstream depends on it
// surviving. A settings row would buy durability nobody wants and pay for it
// with a write per probe.
//
// ORDER IS COMPLETION ORDER, and the lines carry the tier in `harness` so the
// terminal groups them the same way it groups a harness's fixtures.
import type { EvalLogLine } from './surface'

/** Lines kept per model per run. Generous: a probe suite plus a grown
 *  adversarial corpus is well under this, and the sweep's own log is capped
 *  separately at `LIVE_LOG_CAP`. */
const FEED_CAP = 200

const feeds = new Map<string, EvalLogLine[]>()

/** Start a tier's feed over. Called when a tier BEGINS rather than when a run
 *  does, so a resumed run does not replay the previous tier's lines. */
export function startLiveFeed(model: string): void {
  feeds.set(model, [])
}

/** One completed unit of work — a probe, or a provocation. Never throws: this
 *  is telemetry on the hot path of a paid run, and a feed that can break the
 *  thing it reports on is worse than no feed. */
export function noteLive(model: string, line: EvalLogLine): void {
  try {
    const at = feeds.get(model) ?? []
    at.push(line)
    if (at.length > FEED_CAP) at.splice(0, at.length - FEED_CAP)
    feeds.set(model, at)
  } catch {
    /* a live console must never be able to fail a run */
  }
}

/** What tiers 1 and 3 have produced for this model so far. */
export const liveFeedFor = (model: string): EvalLogLine[] => feeds.get(model) ?? []

/** Drop a model's feed — called when its run finishes, so a finished run's
 *  lines cannot leak into the next one's console. */
export function clearLiveFeed(model: string): void {
  feeds.delete(model)
}

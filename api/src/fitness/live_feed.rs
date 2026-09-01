// THE LIVE CONSOLE, FOR THE TIERS THAT ARE NOT THE SWEEP. Port of
// fitness/live-feed.ts.
//
// WHAT WAS MISSING. The fitness terminal is fed from `live_log(sweep.cases)`,
// so it showed tier 2 and nothing else. Start a run with probes and adversarial
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
// So it follows `in_flight` in evals.rs exactly — a module-level map, cleared
// when the tier starts, lost on restart, and nothing downstream depends on it
// surviving. A settings row would buy durability nobody wants and pay for it
// with a write per probe.
//
// ORDER IS COMPLETION ORDER, and the lines carry the tier in `harness` so the
// terminal groups them the same way it groups a harness's fixtures.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::surface::EvalLogLine;

/// Lines kept per model per run. Generous: a probe suite plus a grown
/// adversarial corpus is well under this, and the sweep's own log is capped
/// separately at `LIVE_LOG_CAP`.
const FEED_CAP: usize = 200;

fn feeds() -> &'static Mutex<HashMap<String, Vec<EvalLogLine>>> {
    static FEEDS: OnceLock<Mutex<HashMap<String, Vec<EvalLogLine>>>> = OnceLock::new();
    FEEDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start a tier's feed over. Called when a tier BEGINS rather than when a run
/// does, so a resumed run does not replay the previous tier's lines.
pub fn start_live_feed(model: &str) {
    feeds().lock().expect("the live feed is not contended").insert(model.to_string(), Vec::new());
}

/// One completed unit of work — a probe, or a provocation. Never panics: this
/// is telemetry on the hot path of a paid run, and a feed that can break the
/// thing it reports on is worse than no feed.
pub fn note_live(model: &str, line: EvalLogLine) {
    // A live console must never be able to fail a run. TS wraps the whole body
    // in `try { … } catch {}`; Rust's equivalent for a lock that nobody holding
    // it can poison from telemetry is to carry on without the line.
    if let Ok(mut feeds) = feeds().lock() {
        let at = feeds.entry(model.to_string()).or_default();
        at.push(line);
        let overflow = at.len().saturating_sub(FEED_CAP);
        if overflow > 0 {
            at.drain(0..overflow);
        }
    }
}

/// What tiers 1 and 3 have produced for this model so far.
pub fn live_feed_for(model: &str) -> Vec<EvalLogLine> {
    feeds()
        .lock()
        .map(|feeds| feeds.get(model).cloned().unwrap_or_default())
        .unwrap_or_default()
}

/// Drop a model's feed — called when its run finishes, so a finished run's
/// lines cannot leak into the next one's console.
pub fn clear_live_feed(model: &str) {
    if let Ok(mut feeds) = feeds().lock() {
        feeds.remove(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fitness::surface::LogVerdict;

    fn line(harness: &str, case: &str) -> EvalLogLine {
        EvalLogLine {
            harness: harness.into(),
            case: case.into(),
            verdict: LogVerdict::Pass,
            ms: 10,
            tokens: 20,
            calls: 0,
            up: None,
            note: None,
        }
    }

    /// Each test gets its own model key, because the feed is a process-wide
    /// map and tests run in one process — the same reason TS's tests stub the
    /// module map between cases.
    fn key(name: &str) -> String {
        format!("live-feed-test::{name}")
    }

    #[test]
    fn a_feed_starts_empty_and_collects_in_completion_order() {
        let m = key("collects");
        start_live_feed(&m);
        assert!(live_feed_for(&m).is_empty());
        note_live(&m, line("probe:json", "one"));
        note_live(&m, line("probe:tools", "two"));
        let feed = live_feed_for(&m);
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].case, "one", "newest last: a log reads downward");
        assert_eq!(feed[1].case, "two");
        clear_live_feed(&m);
    }

    #[test]
    fn starting_a_tier_clears_the_previous_tiers_lines() {
        let m = key("restarts");
        start_live_feed(&m);
        note_live(&m, line("probe:json", "one"));
        start_live_feed(&m);
        assert!(live_feed_for(&m).is_empty(), "a resumed run does not replay the last tier");
        clear_live_feed(&m);
    }

    #[test]
    fn a_model_that_never_started_reads_empty() {
        assert!(live_feed_for(&key("never")).is_empty());
    }

    #[test]
    fn the_cap_drops_the_oldest_lines_not_the_newest() {
        let m = key("cap");
        start_live_feed(&m);
        for i in 0..(FEED_CAP + 25) {
            note_live(&m, line("probe:json", &format!("case-{i}")));
        }
        let feed = live_feed_for(&m);
        assert_eq!(feed.len(), FEED_CAP);
        assert_eq!(feed[0].case, format!("case-25"), "the oldest beyond the cap are gone");
        assert_eq!(feed.last().unwrap().case, format!("case-{}", FEED_CAP + 24));
        clear_live_feed(&m);
    }

    #[test]
    fn one_models_feed_cannot_leak_into_anothers() {
        let a = key("isolate-a");
        let b = key("isolate-b");
        start_live_feed(&a);
        note_live(&a, line("probe:json", "only-a"));
        assert!(live_feed_for(&b).is_empty());
        clear_live_feed(&a);
        assert!(live_feed_for(&a).is_empty());
    }
}

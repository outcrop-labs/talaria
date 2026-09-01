// THE SURFACE — port of fitness/surface.ts. WHAT THE ADMIN PAGE READS: the
// index, the record, the value view, the health summary, the transcripts, the
// live console's tier-2 half. The engines underneath (probes, the sweep,
// adversarial, score) produce; this module shapes what a panel consumes.
//
// CROSSING IN SLICES, because the file is the widest in the family: this first
// slice is the LIVE-LOG VOCABULARY — `EvalLogLine`, the shape every tier's
// console lines share and the one thing probes, adversarial and live-feed all
// import from here. The rest (FitnessIndex, the record, the pricing derivations,
// the endpoints' assembly) crosses with the routes that serve it, in this
// slice's last commit.

use serde::Serialize;

/// What happened to one case, in the vocabulary the terminal colours by.
///
/// `Gap` is OURS — the fixture could not fairly ask its question — and is
/// deliberately its own verdict rather than folded into `Fail`: a model is
/// never worse for a gap, and a sweep that gapped half its fixtures is a
/// different problem from one that failed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogVerdict {
    Pass,
    Fail,
    Gap,
    Skip,
    Timeout,
    Error,
}

impl LogVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            LogVerdict::Pass => "pass",
            LogVerdict::Fail => "fail",
            LogVerdict::Gap => "gap",
            LogVerdict::Skip => "skip",
            LogVerdict::Timeout => "timeout",
            LogVerdict::Error => "error",
        }
    }
}

/// UPSTREAM CALLS a case made, and how many never came back. The two numbers
/// that turn a timeout from a symptom into a diagnosis: `1/1 open` is a request
/// that hung, `4/0 open` is a case that spent its budget on retries, and `0`
/// is time that went somewhere before the provider.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct UpCalls {
    pub calls: i64,
    pub open: i64,
}

/// Every case that has landed, reduced to the fields a terminal line needs.
/// ~90 bytes each, so the whole 250-fixture sweep is about 20KB — an order of
/// magnitude under what one failed case's transcript costs.
///
/// NEWEST LAST, because it is a log and a log reads downward.
#[derive(Debug, Clone, Serialize)]
pub struct EvalLogLine {
    pub harness: String,
    pub case: String,
    pub verdict: LogVerdict,
    pub ms: i64,
    pub tokens: i64,
    /// Tool calls the case made, for a dry run. Zero elsewhere.
    pub calls: i64,
    /// Upstream calls, when the case made any. None on every case that never
    /// reached the provider.
    pub up: Option<UpCalls>,
    /// The fixture's own sentence, when there is one to show.
    pub note: Option<String>,
}

/// Lines kept in the feed. A sweep is ~250 fixtures, so this holds an entire
/// run and only bites on a resumed sweep that has already run several times.
pub const LIVE_LOG_CAP: usize = 400;

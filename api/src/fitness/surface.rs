// THE SURFACE — port of fitness/surface.ts. WHAT THE ADMIN PAGE READS: the
// index, the record, the value view, the health summary, the transcripts, the
// live console's tier-2 half. The engines underneath (probes, the sweep,
// adversarial, score) produce; this module shapes what a panel consumes.
//
// CROSSING IN SLICES, because the file is the widest in the family. The first
// slice was the LIVE-LOG VOCABULARY — `EvalLogLine`, the shape every tier's
// console lines share. This slice adds the INDEX VOCABULARY: `FitnessIndex`,
// its entry, and the pricing/budget types the value view and the record store
// are written in. The store itself (the record, the pricing derivations, the
// endpoints' assembly) crosses with the routes that serve it, in this slice's
// last commit.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::fitness::score::FitnessBand;
use crate::fitness::value::HarnessSummary;

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

// ── The index vocabulary ─────────────────────────────────────────────────────

/// One of the three measurement passes, and the only ordering the page has:
/// probes establish what a model can reach, evals establish what it does with
/// it, adversarial establishes what it does under pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TierId {
    Probes,
    Evals,
    Adversarial,
}

impl TierId {
    pub fn as_str(self) -> &'static str {
        match self {
            TierId::Probes => "probes",
            TierId::Evals => "evals",
            TierId::Adversarial => "adversarial",
        }
    }
}

pub const TIER_IDS: [TierId; 3] = [TierId::Probes, TierId::Evals, TierId::Adversarial];

pub fn is_tier_id(v: &str) -> bool {
    TIER_IDS.iter().any(|t| t.as_str() == v)
}

/// The speed half of a run, computed once where the sweep is scored. Pure, so
/// the matrix column and the report card cannot disagree about the same run —
/// the derivation crosses with the store.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedReading {
    /// THE HEADLINE: output tokens per second, median over the cases this pass
    /// measured.
    ///
    /// WHY NOT TIME-PER-CASE, which this used to lead with. A fixture that
    /// asks for a SKILL.md takes longer than one that asks for a chat title on
    /// any model, so a per-case latency is mostly a fact about which fixtures
    /// ran — and it moves whenever the corpus does. Tokens per second divides
    /// that out: it is the rate the model generates at, comparable between two
    /// models that ran different fixtures and between two runs of a corpus
    /// that changed.
    ///
    /// None when nothing measured both a duration and a completion, which is
    /// the honest answer for a sweep of contract failures.
    pub tokens_per_second: Option<f64>,
    /// Median per-case latency, ms — the runner's own measure, so it is the
    /// same number `harness_runs` records and observed-vs-tested compares
    /// against. Kept beside the rate because "how fast does it generate" and
    /// "how long do I wait for a fixture" are both real questions.
    pub p50: i64,
    pub p95: i64,
    /// Wall clock of the whole sweep, from the first case starting to the last
    /// one finishing. NOT the sum of the latencies: under concurrency the run
    /// is shorter than its parts, and with retries a case costs more than it
    /// says.
    pub elapsed_ms: i64,
    /// Fixtures per minute — the figure that answers "how long will testing
    /// the next candidate take me".
    pub per_minute: f64,
    /// Cases in flight while this was measured. See `FitnessIndexEntry.speed`.
    pub concurrency: i64,
    /// HOW MANY CASES THIS READING IS OVER. A supplemental pass of seven
    /// fixtures gives a real but small sample, and a median over seven is a
    /// different claim from a median over two hundred and forty — the panel
    /// says which.
    pub sample: i64,
}

/// One cell of the matrix: slot key → the verdict that colors it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessCell {
    pub band: FitnessBand,
    /// `SlotVerdict.reasons[0].detail`, which score.rs sorts worst-band-first
    /// precisely so this is the right one.
    pub reason: Option<String>,
}

/// Tier 3's verdict — a fact about the MODEL rather than about a slot, so it
/// never colors a cell and is carried apart from them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyCell {
    pub band: FitnessBand,
    pub resistance: Option<f64>,
}

/// THE MATRIX ROW, and the only thing the matrix read touches. Kept apart from
/// the full report so drawing 30 models × 20 slots is ONE settings read rather
/// than thirty multi-hundred-kilobyte ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessIndexEntry {
    pub model: String,
    /// When the run that produced these bands finished. The matrix prints it —
    /// a verdict with no date is a verdict an admin cannot judge the age of.
    pub at: String,
    pub tiers: Vec<TierId>,
    pub guarded: bool,
    pub cells: HashMap<String, FitnessCell>,
    pub safety: Option<SafetyCell>,
    pub probes_wrote: i64,
    /// HOW FAST THIS MODEL IS IN THIS INSTALL, measured over the same 247
    /// fixtures every candidate runs — which is what makes the number
    /// comparable down a column at all. None when tier 2 did not run.
    ///
    /// IT CARRIES THE WIDTH IT WAS MEASURED AT, and that is not decoration: at
    /// four in flight a per-case latency includes queueing at the provider, so
    /// a p50 from a 4-wide sweep and a p50 from a sequential one are two
    /// different measurements wearing one name. The column says so rather than
    /// letting an admin compare them silently.
    pub speed: Option<SpeedReading>,
    pub cost_usd: Option<f64>,
    pub calls: i64,
    /// The sweep did not finish (stopped, or interrupted by a deploy). Every
    /// unrun harness is already `untested` in the cells; this says the RUN was
    /// partial so the page can say so once, at the top, instead of the admin
    /// inferring it from a scatter of grey.
    pub partial: bool,
    /// PER HARNESS, the small half of the report — what the value view needs
    /// to weigh a verdict against how much of a real day that harness is,
    /// without reading two dozen multi-hundred-kilobyte records to draw one
    /// table.
    ///
    /// `band` is `harness_bands`' worst-across-slots collapse, and covers the
    /// UNBOUND harnesses the cells cannot speak for. `prompt`/`completion` are
    /// per-case tokens THIS model spent, which is the only basis on which a
    /// terse model and a chatty one price differently.
    ///
    /// OPTIONAL, and stays that way: entries archived before this field
    /// existed are still valid rows. `value.rs` backfills one from the full
    /// report on read, and degrades to the cells and the shared token budget
    /// if even that is gone — never a demand to re-test a model an admin
    /// already paid for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harnesses: Option<HashMap<String, HarnessSummary>>,
}

/// Keyed by the id the CATALOG offers — see `stored_id_for` (the store slice)
/// for how an id the run used maps onto it.
pub type FitnessIndex = HashMap<String, FitnessIndexEntry>;

// ── Pricing ──────────────────────────────────────────────────────────────────

/// $/MTok, both directions. The field names are Rust's; the payload keeps the
/// TS `in`/`out` spellings, which every consumer already reads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    #[serde(rename = "in")]
    pub in_per_mtok: f64,
    #[serde(rename = "out")]
    pub out_per_mtok: f64,
}

/// One harness's per-case token shape as the global budget records it —
/// measured from whichever candidate last swept it, so it is right about the
/// prompt (the fixtures are fixed) and only approximately right about the
/// completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenBudgetEntry {
    pub prompt: i64,
    pub completion: i64,
    pub at: String,
}

/// harness id → the per-case tokens something has measured for it. The shared
/// floor the value view prices an untested model's harnesses from, and the
/// thing a new sweep makes more truthful.
pub type TokenBudget = std::collections::BTreeMap<String, TokenBudgetEntry>;

// THE SURFACE — WHAT THE ADMIN PAGE READS: the
// index, the record, the value view, the health summary, the transcripts, the
// live console's tier-2 half. The engines underneath (probes, the sweep,
// adversarial, score) produce; this module shapes what a panel consumes.
//
// LAID OUT IN SECTIONS, because the file is the widest in the family: the
// live-log vocabulary first, then the index vocabulary, then the store.
// The record, the run, the estimate, the capability rows, the archive's
// re-keying and eviction, the stop plumbing, and the reads the admin route
// serializes all live here and nowhere else. THIS FILE ORCHESTRATES; IT
// SCORES NOTHING — every band comes from score.rs, every fact from
// capability.rs, every production number from observed.rs.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fitness::score::FitnessBand;
use crate::fitness::value::HarnessSummary;
use crate::harness::run::BoxFut;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
/// the derivation lives with the store.
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
///
/// ORDERED, not a `HashMap`: the index rides the wire straight out of
/// `app_settings` (the matrix view sends it whole, the health view lists its
/// keys), and the read sends the keys in the order Postgres kept them. An
/// `IndexMap` keeps insertion order like a JS object. (serde_json's own
/// ordered map cannot carry a typed entry: its
/// methods exist only on `Map<String, Value>`.)
pub type FitnessIndex = indexmap::IndexMap<String, FitnessIndexEntry>;

// ── Pricing ──────────────────────────────────────────────────────────────────

/// $/MTok, both directions. The field names are Rust's; the payload keeps the
/// wire's `in`/`out` spellings, which every consumer already reads.
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

// ── The archive ──────────────────────────────────────────────────────────────

/// The settings row every stored piece of the fitness plane lives in — one
/// spelling, so rows written by earlier deploys stay legible.
pub const INDEX_KEY: &str = "model_fitness_index";
/// THE RUN STATUS STORE, keyed by candidate — a NEW key rather than a reshape
/// of the old one: the status row of a run already in flight is written by
/// code already loaded, and a shape changed under it loses the row.
/// `STATUS_KEY` is still READ, folded in under its own model, and nothing
/// writes it again.
pub const RUNS_KEY: &str = "model_fitness_runs";

/// MODELS ASKED TO STOP, IN THE DATABASE.
///
/// THE STOP BUTTON DID NOT WORK, and this is why. The request was a boolean on
/// an in-process map, so it could only ever reach a run whose closure lived in
/// the module instance the request happened to hit. In dev that is one HMR
/// reload away from being a different instance — a sweep started before a
/// server-side edit had a Stop button that returned `stopped: false` while the
/// run carried on for another twenty minutes. The same hole exists across a
/// restart, and across processes on any deployment with more than one.
///
/// A settings row is the one thing every instance can see. The in-process flag
/// stays as the fast path (a run in THIS instance stops on the next case
/// without a read); this is what makes the request survive everything else.
pub const STOP_KEY: &str = "model_fitness_stop";
pub const STATUS_KEY: &str = "model_fitness_status";
pub const BUDGET_KEY: &str = "model_fitness_budget";

/// The settings row one model's full report lives under.
pub fn record_key(model: &str) -> String {
    format!("model_fitness_report:{model}")
}

/// The sweep half a RECORD keeps — a summary of the run, not the resumable
/// status. Deliberately its own shape: the record is written once at the end,
/// while `EvalSweepStatus` keeps mutating, and conflating them is how a
/// finished record comes to claim a sweep is still running.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessSweepSummary {
    pub state: String,
    pub done: i64,
    pub total: i64,
    pub error: Option<String>,
    /// Older sweeps did not record which registered harnesses were fixture-less.
    #[serde(default)]
    pub unfixtured: Vec<String>,
    /// HOW WIDE IT RAN, archived with the run. Without it a p50 from a 4-wide
    /// sweep and a p50 from a sequential one are the same field holding two
    /// different measurements, and the page would compare them silently.
    pub concurrency: crate::fitness::evals::SweepConcurrency,
}

/// The full report for one model. One settings row per model, fetched only
/// when an admin opens that model.
///
/// EVERY FIELD ADDED AFTER A RUN WAS ARCHIVED IS MISSING FROM THAT RUN —
/// `upgrade_record` normalizes on read, in one place, rather than every
/// consumer guarding every field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessRecord {
    pub model: String,
    pub at: String,
    pub tiers: Vec<TierId>,
    pub report: crate::fitness::score::FitnessReport,
    pub harnesses: Vec<crate::fitness::evals::HarnessScore>,
    /// Drill-down cases. Bounded — see [`DRILLDOWN_CAP`].
    pub cases: Vec<crate::fitness::evals::EvalCaseScore>,
    pub dropped_cases: i64,
    pub probes: Option<crate::fitness::probes::ProbeReport>,
    pub adversarial: Option<crate::fitness::adversarial::AdversarialReport>,
    /// WHEN EACH TIER WAS LAST MEASURED. A record is merged across runs — a
    /// tier that did not run keeps its previous result — so `at` alone would
    /// put today's date over a month-old probe result.
    #[serde(default)]
    pub tier_at: HashMap<TierId, String>,
    pub sweep: FitnessSweepSummary,
}

/// `low` arrived with the two-way valve. Before it, the valve only ever
/// closed, so the width a run ENDED at was also the narrowest it ever reached
/// — which makes `ended` the correct backfill rather than a guess.
fn upgrade_concurrency(
    c: Option<crate::fitness::evals::SweepConcurrency>,
) -> crate::fitness::evals::SweepConcurrency {
    match c {
        None => crate::fitness::evals::SweepConcurrency {
            requested: 1,
            ended: 1,
            low: 1,
            narrowed_because: None,
        },
        Some(mut c) => {
            if c.low == 0 {
                c.low = c.ended;
            }
            c
        }
    }
}

/// AN ARCHIVED RECORD WAS WRITTEN BY AN OLDER VERSION, and it is read by the
/// current one. Every field added to `FitnessRecord` after a run was archived
/// is missing from that run, and the type says otherwise — so a panel that
/// reads `record.sweep.concurrency.low` throws on a report from last week and
/// takes the whole route down with it. THAT IS EXACTLY WHAT HAPPENED. So the
/// archive is upgraded ON READ, in one place.
pub fn upgrade_record(record: Option<FitnessRecord>) -> Option<FitnessRecord> {
    let mut record = record?;
    record.sweep.concurrency = upgrade_concurrency(Some(record.sweep.concurrency));
    // Older archives predate per-tier stamps; the run's own date is the honest
    // answer for every tier it contains.
    if record.tier_at.is_empty() {
        record.tier_at = record
            .tiers
            .iter()
            .map(|t| (*t, record.at.clone()))
            .collect();
    }
    Some(record)
}

/// `upgrade_record`, but on the WIRE VALUE — the detail view hands the archived
/// record to the browser, and the archive is jsonb: Postgres stores an object's
/// keys ordered by (length, then bytes), the jsonb read hands that order
/// back, and `{ ...record, … }` keeps every stored key where it was. A typed
/// struct re-serializes in declaration order — same keys, different bytes. So
/// the drill-down upgrades the raw `Value` instead: `Map::insert` on an
/// existing key replaces the value in place (JS spread semantics, which
/// `preserve_order` gives serde_json's map), and only keys the archive
/// predates are appended.
fn upgrade_record_value(raw: Value) -> Value {
    let Some(mut record) = raw.as_object().cloned() else {
        return raw; // not an object — the typed parse downstream fails the way the old read did
    };
    // sweep: { ...record.sweep, concurrency: upgradeConcurrency(...), unfixtured: ?? [] }
    let mut sweep = record
        .get("sweep")
        .and_then(|s| s.as_object().cloned())
        .unwrap_or_default();
    sweep.insert(
        "concurrency".into(),
        upgrade_concurrency_value(sweep.get("concurrency")),
    );
    sweep.entry("unfixtured").or_insert(serde_json::json!([]));
    record.insert("sweep".into(), Value::Object(sweep));
    // tierAt: record.tierAt ?? fromEntries(tiers.map(t => [t, at]))
    if record.get("tierAt").map(|v| v.is_null()).unwrap_or(true) {
        let at = record.get("at").cloned().unwrap_or(Value::Null);
        let mut stamps = serde_json::Map::new();
        for t in record
            .get("tiers")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default()
        {
            if let Some(tier) = t.as_str() {
                stamps.insert(tier.to_string(), at.clone());
            }
        }
        record.insert("tierAt".into(), Value::Object(stamps));
    }
    // The arrays themselves are not optional at the call sites, only their fields.
    if record.get("cases").map(|v| v.is_null()).unwrap_or(true) {
        record.insert("cases".into(), serde_json::json!([]));
    }
    if record.get("harnesses").map(|v| v.is_null()).unwrap_or(true) {
        record.insert("harnesses".into(), serde_json::json!([]));
    }
    Value::Object(record)
}

/// The `??` in `c.low ?? c.ended`, on the wire value: missing or null backfills
/// from `ended`, and an `ended` that is itself missing drops the key — JS
/// `undefined` values do not survive `JSON.stringify`, and neither may we.
fn upgrade_concurrency_value(c: Option<&Value>) -> Value {
    let Some(c) = c.filter(|v| !v.is_null()) else {
        return serde_json::json!({ "requested": 1, "ended": 1, "low": 1, "narrowedBecause": null });
    };
    let Some(conc) = c.as_object().cloned() else {
        return c.clone();
    };
    let mut conc = conc;
    let backfill = conc.get("ended").cloned();
    match (
        conc.get("low").map(|l| l.is_null()).unwrap_or(true),
        backfill,
    ) {
        (true, Some(ended)) => {
            conc.insert("low".into(), ended);
        }
        (true, None) => {
            conc.remove("low");
        }
        (false, _) => {}
    }
    Value::Object(conc)
}

/// THE LONG-RUN STATUS, in `app_settings`, deliberately the same shape and
/// lifecycle as the reindex status. Talaria has one long-run mechanism; a
/// second would be a second set of stuck-state bugs. Tier 2's own
/// `EvalSweepStatus` carries the case counter and is merged in on read rather
/// than copied here — two progress counters for one run is how they come to
/// disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessRunStatus {
    pub state: FitnessRunState,
    pub model: Option<String>,
    pub tiers: Vec<TierId>,
    /// Which tier is in flight, or `scoring`, or nothing.
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// LAST SIGN OF LIFE. A run is a background task inside a process, and
    /// the status that says `running` outlives the process — so a restart
    /// used to leave a row claiming to run forever, with the console counting
    /// it against the concurrency limit and Stop writing a request nothing
    /// would ever read.
    ///
    /// Written whenever the run touches its status, which is at every phase
    /// boundary. Absent on rows written before this existed; `stale_run`
    /// treats that as `started_at`, so old rows age out rather than being
    /// trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FitnessRunState {
    Idle,
    Running,
    Done,
    Error,
}

impl FitnessRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            FitnessRunState::Idle => "idle",
            FitnessRunState::Running => "running",
            FitnessRunState::Done => "done",
            FitnessRunState::Error => "error",
        }
    }
}

// One run's status with tier 2's counter folded in stays a raw `Value` —
// see `fitness_runs` for why (the stored row's key order is part of the wire
// contract, and a typed struct cannot promise it).

/// Every run the page draws, newest first, plus what the panel needs to know
/// about whether it may start another.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessRunsView {
    /// The wire rows, built from the raw blob so each row keeps its stored
    /// jsonb key order — see `fitness_runs`.
    pub runs: Vec<Value>,
    /// How many may run at once, sent rather than restated in a Svelte file.
    pub max: usize,
    /// True when a further Start would be refused.
    pub full: bool,
}

pub fn idle_status() -> FitnessRunStatus {
    FitnessRunStatus {
        state: FitnessRunState::Idle,
        model: None,
        tiers: Vec::new(),
        phase: None,
        started_at: None,
        finished_at: None,
        error: None,
        heartbeat_at: None,
    }
}

/// Drill-down cases kept per report. A sweep produces ~70; keeping every
/// failing transcript for two dozen models turns `app_settings` into a
/// transcript archive. Failed cases come first (they are the ones that carry
/// a prompt and a reply at all); clean rows are cheap and are all kept.
pub const DRILLDOWN_CAP: usize = 30;
/// Models kept in the archive. A model nobody has tested in two dozen swaps is
/// a model whose verdict is about weights that have since moved.
pub const KEEP_MODELS: usize = 24;

/// THE LIVE LIST, which is not the archived one. [`drilldown`] keeps every
/// clean case because they are cheap and an archive is read once. This is
/// polled every three seconds while a sweep runs, and a clean case carries
/// nothing the progress counter does not already say — so the live view sends
/// what an admin opened the modal to watch: everything that FAILED something,
/// plus the last few so the list visibly moves. Measured on a real sweep: 155
/// cases a poll became 34.
pub const LIVE_RECENT: usize = 6;

/// HOW MANY CANDIDATES MAY BE TESTED AT ONCE.
///
/// EIGHT, RAISED FROM THREE once the checkpoint stopped being shared. Three
/// was never about the provider — each run is strictly sequential inside
/// itself, so N runs are N concurrent requests and eight of those is nothing.
/// It was about US: the resume checkpoint used to live in one settings row
/// holding every running candidate's cases, so each per-case write was a
/// read-modify-write of every sibling's work too. Write traffic went as
/// O(N² x cases²); one row per candidate took N out of the cost entirely.
///
/// WHAT BINDS NOW IS THE PROVIDER, not this process. Measured at ~16k tokens
/// a minute per run, so eight is ~130k TPM against one key — comfortably
/// inside a paid tier and the point where a single key starts to throttle.
pub const MAX_CONCURRENT_RUNS: usize = 8;

/// Speed, from the cases a sweep recorded. Pure, so the matrix column and the
/// report card cannot disagree about the same run.
pub fn speed_of(
    cases: &[crate::fitness::evals::EvalCaseScore],
    concurrency: usize,
) -> Option<SpeedReading> {
    // OVER THE CASES THIS PASS RAN, never over the whole ledger — see
    // `EvalSweep.measured`. A supplemental pass that ran seven fixtures must
    // not report a latency computed from two hundred and forty inherited ones
    // measured last week at a different width.
    //
    // MEASURED CASES ONLY. A skip never called the model and a case the
    // provider never answered has no latency to speak of; averaging their
    // zeros in would make a badly-served model look fast.
    let scored: Vec<&crate::fitness::evals::EvalCaseScore> = cases
        .iter()
        .filter(|c| c.skipped.is_none() && c.latency_ms > 0)
        .collect();
    if scored.is_empty() {
        return None;
    }
    let mut sorted: Vec<i64> = scored.iter().map(|c| c.latency_ms).collect();
    sorted.sort_unstable();
    let at = |q: f64| -> i64 {
        let idx = ((q * sorted.len() as f64).ceil() as i64 - 1).clamp(0, sorted.len() as i64 - 1);
        sorted[idx as usize]
    };
    // PER CASE, THEN THE MEDIAN — not total tokens over total time. Under
    // concurrency the wall clock overlaps, so an aggregate would report a rate
    // no single request ever achieved; and one long generation would dominate
    // the sum. Each case's own rate is a fact about that request, and the
    // median of them is a fact about the model.
    let mut rates: Vec<f64> = scored
        .iter()
        .filter(|c| c.completion_tokens > 0 && c.latency_ms > 0)
        .map(|c| c.completion_tokens as f64 / (c.latency_ms as f64 / 1000.0))
        .collect();
    rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let tokens_per_second = if rates.is_empty() {
        None
    } else {
        Some((rates[(rates.len() - 1) / 2] * 10.0).round() / 10.0)
    };
    let starts: Vec<i64> = cases
        .iter()
        .filter_map(|c| crate::agent_auth::iso_to_epoch_ms(&c.started_at))
        .filter(|n| *n > 0)
        .collect();
    let ends: Vec<i64> = cases
        .iter()
        .filter_map(|c| crate::agent_auth::iso_to_epoch_ms(&c.started_at).map(|n| n + c.wall_ms))
        .filter(|n| *n > 0)
        .collect();
    let elapsed_ms = if !starts.is_empty() && !ends.is_empty() {
        (ends.iter().max().unwrap_or(&0) - starts.iter().min().unwrap_or(&0)).max(0)
    } else {
        0
    };
    Some(SpeedReading {
        tokens_per_second,
        p50: at(0.5),
        p95: at(0.95),
        elapsed_ms,
        per_minute: if elapsed_ms > 0 {
            ((cases.len() as f64 / elapsed_ms as f64) * 60_000.0 * 10.0).round() / 10.0
        } else {
            0.0
        },
        concurrency: concurrency as i64,
        sample: scored.len() as i64,
    })
}

/// THE LIVE FEED'S ONE LINE PER CASE. Same verdict vocabulary the transcript
/// table writes — see [`crate::fitness::transcripts::verdict_of`].
pub fn live_log(cases: &[crate::fitness::evals::EvalCaseScore]) -> Vec<EvalLogLine> {
    cases
        .iter()
        .rev()
        .take(LIVE_LOG_CAP)
        .rev()
        .map(|c| {
            let verdict = crate::fitness::transcripts::verdict_of(c);
            let verdict = match verdict {
                "pass" => LogVerdict::Pass,
                "fail" => LogVerdict::Fail,
                "gap" => LogVerdict::Gap,
                "skip" => LogVerdict::Skip,
                "timeout" => LogVerdict::Timeout,
                _ => LogVerdict::Error,
            };
            let up = c
                .upstream
                .as_ref()
                .filter(|u| !u.is_empty())
                .map(|u| UpCalls {
                    calls: u.len() as i64,
                    open: u.iter().filter(|a| !a.settled).count() as i64,
                });
            EvalLogLine {
                harness: c.harness.clone(),
                case: c.case.clone(),
                verdict,
                ms: c.latency_ms,
                tokens: c.prompt_tokens + c.completion_tokens,
                calls: c.calls.as_ref().map(|v| v.len() as i64).unwrap_or(0),
                up,
                // The reason, whichever kind it is, capped to a terminal line.
                // `error` first: when a case both errored and failed its check,
                // the error is the cause.
                note: c
                    .error
                    .as_deref()
                    .or(c.gap.as_deref())
                    .or(c.task_error.as_deref())
                    .or(c.skipped.as_deref())
                    .map(|n| crate::body::truncate_utf16(n, 200).to_string()),
            }
        })
        .collect()
}

/// THE TIER-1 AND TIER-3 CONSOLE LINES FOR A FINISHED RUN.
///
/// The live feed is in memory and does not survive a restart, but the ARCHIVE
/// does: `probes.results` and `adversarial.cases` are the same units the
/// console printed while the run was going. Rebuilding from them is what makes
/// the console outlive the process rather than only the run.
///
/// THE FEED WINS WHILE IT LASTS, because it carries per-unit timings the
/// archived reports never stored — a probe that took 87 seconds is worth
/// seeing, and rebuilding it from the record can only report 0ms.
fn archived_tier_log(record: Option<&FitnessRecord>, feed: &[EvalLogLine]) -> Vec<EvalLogLine> {
    if !feed.is_empty() {
        return feed.to_vec();
    }
    let Some(record) = record else {
        return Vec::new();
    };
    let mut out: Vec<EvalLogLine> = record
        .probes
        .as_ref()
        .map(|p| {
            p.results
                .iter()
                .map(|r| crate::fitness::probes::probe_line(r, 0))
                .collect()
        })
        .unwrap_or_default();
    out.extend(
        record
            .adversarial
            .as_ref()
            .map(|a| {
                a.cases
                    .iter()
                    .map(|c| crate::fitness::adversarial::provocation_line(c, 0))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    out
}

/// THE WHOLE RUN, IN THE ORDER IT HAPPENED — probes, then fixtures, then
/// provocations.
///
/// IT USED TO BE "sweep, then everything else", which is not an order at all.
/// Tier 1 runs FIRST and its lines were printed UNDERNEATH the two hundred
/// fixtures that ran after them, so the console read as a timeline that was
/// not one — and a console a watcher has to distrust is worse than no console.
///
/// BY TIER RATHER THAN BY TIMESTAMP, and that is deliberate. `run_fitness`
/// runs the three in sequence, so tier order IS chronological order; and
/// unlike a timestamp it survives into the ARCHIVE, where probe results and
/// provocation scores carry no clock of their own. One ordering rule that
/// works live and after the fact beats two that disagree the moment a run
/// finishes.
pub fn run_log(
    cases: &[crate::fitness::evals::EvalCaseScore],
    tiers: &[EvalLogLine],
) -> Vec<EvalLogLine> {
    let of = |harness: &str| -> Vec<EvalLogLine> {
        tiers
            .iter()
            .filter(|l| l.harness == harness)
            .cloned()
            .collect()
    };
    let mut out = of("probes");
    out.extend(live_log(cases));
    out.extend(of("adversarial"));
    out
}

/// The live modal's bounded case list — see [`LIVE_RECENT`].
pub fn live_cases(
    cases: &[crate::fitness::evals::EvalCaseScore],
) -> (Vec<crate::fitness::evals::EvalCaseScore>, i64) {
    let bad = |c: &crate::fitness::evals::EvalCaseScore| {
        c.skipped.is_none()
            && (c.task == crate::fitness::evals::TaskVerdict::Fail
                || !c.contract_held
                || c.timed_out)
    };
    let failed: Vec<_> = cases.iter().filter(|c| bad(c)).cloned().collect();
    let recent: Vec<_> = cases
        .iter()
        .rev()
        .take(LIVE_RECENT)
        .filter(|c| !bad(c))
        .cloned()
        .collect();
    let mut kept: Vec<_> = failed.into_iter().rev().take(DRILLDOWN_CAP).rev().collect();
    kept.extend(recent);
    let dropped = cases.len() as i64 - kept.len() as i64;
    (kept, dropped)
}

/// THE CAP APPLIES TO TRANSCRIPTS, NOT TO CASES.
///
/// `EvalCaseScore` carries a prompt and a reply only for cases that failed
/// something, and those are the only rows with any size to them. Every clean
/// case is a handful of numbers and is kept whole — dropping them would leave
/// the panel unable to say how many fixtures actually passed, which is a worse
/// trade than a bounded settings row. `dropped` therefore counts transcripts
/// an admin cannot see, and nothing else.
pub fn drilldown(
    cases: &[crate::fitness::evals::EvalCaseScore],
    cap: usize,
) -> (Vec<crate::fitness::evals::EvalCaseScore>, i64) {
    let heavy = |c: &crate::fitness::evals::EvalCaseScore| c.prompt.is_some() || c.raw.is_some();
    let with_transcript: Vec<_> = cases.iter().filter(|c| heavy(c)).cloned().collect();
    let dropped = with_transcript.len().saturating_sub(cap) as i64;
    let mut kept: Vec<_> = with_transcript.into_iter().take(cap).collect();
    kept.extend(cases.iter().filter(|c| !heavy(c)).cloned());
    (kept, dropped)
}

/// Newest `keep` models survive; the rest are named so their report rows can
/// go with them — an orphaned report row is a settings row nothing will ever
/// read again.
///
/// Pure, and returns a NEW index rather than mutating: the caller writes the
/// index and the report deletions in one step, and a half-applied eviction
/// that had already mutated the caller's object is a matrix listing models
/// whose reports are gone.
pub fn evict_archive(index: &FitnessIndex, keep: usize) -> (FitnessIndex, Vec<String>) {
    let mut ordered: Vec<&FitnessIndexEntry> = index.values().collect();
    ordered.sort_by(|a, b| b.at.cmp(&a.at));
    let evicted: Vec<String> = ordered
        .into_iter()
        .skip(keep)
        .map(|e| e.model.clone())
        .collect();
    let mut kept = index.clone();
    for model in &evicted {
        // shift_remove, not the deprecated remove/swap_remove: eviction keeps
        // the surviving keys in the stored order the wire rides (see the
        // `FitnessIndex` alias), and a swap would reorder them.
        kept.shift_remove(model);
    }
    (kept, evicted)
}

/// The budget a sweep leaves behind. Pure, and separated from the read/write
/// for the reason the zero rule below is worth a test of its own.
pub fn next_budget(
    prev: &TokenBudget,
    harnesses: &[crate::fitness::evals::HarnessScore],
    at: &str,
) -> TokenBudget {
    let mut budget = prev.clone();
    for h in harnesses {
        if h.cases == 0 {
            continue;
        }
        // ZERO TOKENS IS NOT A MEASUREMENT OF ZERO, and writing it as one cost
        // a real install its whole budget: a sweep against a model id the
        // gateway could not reach ran all 70 cases, failed every one before a
        // single token moved, and overwrote 26 harnesses' good numbers with 0.
        // Every dollar figure downstream — the tier-2 estimate, the value
        // view's daily cost — then read $0.00 for every model on the page. A
        // run that measured nothing leaves the previous measurement where it
        // is.
        if h.prompt_tokens + h.completion_tokens == 0 {
            continue;
        }
        budget.insert(
            h.meta.id.clone(),
            TokenBudgetEntry {
                prompt: (h.prompt_tokens as f64 / h.cases as f64).round() as i64,
                completion: (h.completion_tokens as f64 / h.cases as f64).round() as i64,
                at: at.to_string(),
            },
        );
    }
    budget
}

/// An empty sweep, for a run that skipped tier 2.
///
/// Every bound harness then lands on `not-swept` → `untested`, which is the
/// correct reading: probes alone can turn a cell RED (a required capability
/// came back false) but can never turn one green. A page that showed probe
/// passes as Ready would be claiming the harnesses were exercised.
pub fn empty_sweep(
    model: &str,
    unfixtured: Vec<String>,
    guarded: bool,
    at: &str,
) -> crate::fitness::evals::EvalSweep {
    crate::fitness::evals::EvalSweep {
        model: model.to_string(),
        // No cases ran, so there is no width to report and 1 is the honest
        // reading.
        concurrency: crate::fitness::evals::SweepConcurrency {
            requested: 1,
            ended: 1,
            low: 1,
            narrowed_because: None,
        },
        measured: Vec::new(),
        state: crate::fitness::evals::EvalSweepState::Idle,
        started_at: Some(at.to_string()),
        finished_at: Some(at.to_string()),
        done: 0,
        total: 0,
        error: None,
        harnesses: Vec::new(),
        cases: Vec::new(),
        unfixtured,
        guarded,
    }
}

// ── The deps ─────────────────────────────────────────────────────────────────
//
// Same seam as every engine in the family: the surface is orchestration over
// edges it does not own, and the edges arrive as a struct of boxed closures so
// a test drives the store without a database, a gateway, or a clock. A Rust
// struct has no spread, so the test builders below start from `panic_deps` and
// overwrite exactly the edges they mean to exercise — an untouched edge
// panics with its own name rather than quietly hitting production.

/// The sweep-start half of the `runEvalSweep` edge. The surface itself owns
/// the stop reader and the transcript archiver, so they are not here — a test
/// that drove them through the edge would be testing the test.
#[derive(Default, Clone)]
pub struct SweepStart {
    pub restart: bool,
    pub only: Option<Vec<String>>,
    pub retry_failed: bool,
    pub supplement: bool,
    pub concurrency: Option<usize>,
}

/// Clone is cheap (every edge is an `Arc`) and the value view needs it: its
/// own edges are `'static` closures, so each one takes its own handle to this
/// struct rather than borrowing it.
#[derive(Clone)]
pub struct SurfaceDeps {
    /// The gateway catalog: every callable id, bare and endpoint-qualified.
    pub models: Arc<
        dyn Fn() -> BoxFut<Result<Vec<crate::model::access::GatewayModel>, String>> + Send + Sync,
    >,
    /// Where a model id CAN land, with prices. Used for the estimate only.
    pub routing: Arc<
        dyn Fn(String) -> BoxFut<Result<crate::gateway::registry::ModelRouting, String>>
            + Send
            + Sync,
    >,
    pub capabilities: Arc<
        dyn Fn(String) -> BoxFut<HashMap<String, crate::capability::CapabilityFact>> + Send + Sync,
    >,
    pub forget: Arc<dyn Fn(String) -> BoxFut<Result<(), String>> + Send + Sync>,
    pub harnesses: Arc<
        dyn Fn() -> BoxFut<Result<Vec<crate::harness::registry::RegisteredHarness>, String>>
            + Send
            + Sync,
    >,
    pub bind_slots: Arc<
        dyn Fn(
                Vec<crate::harness::registry::RegisteredHarness>,
            ) -> BoxFut<Result<Vec<crate::fitness::score::SlotBinding>, String>>
            + Send
            + Sync,
    >,
    /// Raw settings read with the fallback contract. Typed reads go through
    /// [`SurfaceDeps::setting`], because a closure field cannot be generic.
    pub read_setting: Arc<dyn Fn(String, Value) -> BoxFut<Result<Value, String>> + Send + Sync>,
    pub write_setting: Arc<dyn Fn(String, Value) -> BoxFut<Result<(), String>> + Send + Sync>,
    pub estimate_probes: Arc<
        dyn Fn(String, bool) -> BoxFut<Result<crate::fitness::probes::ProbeEstimate, String>>
            + Send
            + Sync,
    >,
    pub run_probes: Arc<
        dyn Fn(String, bool) -> BoxFut<Result<crate::fitness::probes::ProbeReport, String>>
            + Send
            + Sync,
    >,
    /// Throw away one candidate's resume ledger — see `clear_fitness_results`.
    pub clear_eval_status: Arc<dyn Fn(String) -> BoxFut<Result<(), String>> + Send + Sync>,
    /// Throw away archived transcripts; None means every model. Returns rows.
    pub clear_transcripts: Arc<dyn Fn(Option<String>) -> BoxFut<Result<i64, String>> + Send + Sync>,
    /// Registered MCP servers, for "the deployment supplies what the model
    /// cannot" — see `supplied_by`.
    pub mcp_servers: Arc<
        dyn Fn() -> BoxFut<Result<Vec<crate::capability_reach::ReachServer>, String>> + Send + Sync,
    >,
    /// Talaria's OWN checked tools, under the registry. Injected rather than
    /// imported at the call site so a matrix test never has to reach SearXNG.
    pub platform_supply: Arc<
        dyn Fn() -> BoxFut<Result<Vec<crate::capability_reach::PlatformSupply>, String>>
            + Send
            + Sync,
    >,
    pub estimate_adversarial: Arc<
        dyn Fn(
                Option<String>,
                Option<crate::fitness::adversarial::PriceFn>,
            )
                -> BoxFut<Result<crate::fitness::adversarial::AdversarialEstimate, String>>
            + Send
            + Sync,
    >,
    pub run_adversarial: Arc<
        dyn Fn(
                String,
                Option<String>,
            )
                -> BoxFut<Result<crate::fitness::adversarial::AdversarialReport, String>>
            + Send
            + Sync,
    >,
    pub run_eval_sweep: Arc<
        dyn Fn(String, SweepStart) -> BoxFut<Result<crate::fitness::evals::EvalSweep, String>>
            + Send
            + Sync,
    >,
    pub eval_sweep_statuses: Arc<
        dyn Fn(
                Vec<String>,
            )
                -> BoxFut<Result<HashMap<String, crate::fitness::evals::EvalSweepStatus>, String>>
            + Send
            + Sync,
    >,
    /// Synchronous by design: the in-process stop set reads no future.
    pub stop_eval_sweep: Arc<dyn Fn(Option<String>) -> bool + Send + Sync>,
    pub guard_config:
        Arc<dyn Fn() -> BoxFut<Result<crate::gateway::guard::GuardConfig, String>> + Send + Sync>,
    /// What this deployment can reach for a model — natively or by tool. The
    /// edge takes the MODEL, not its capability keys: key resolution
    /// (`capabilityKeysFor`) is a database read, and keeping it inside the real
    /// edge is what lets the store run its scoring step without one.
    pub reach: Arc<
        dyn Fn(
                String,
                Vec<String>,
            )
                -> BoxFut<Result<HashMap<String, crate::capability_reach::Reach>, String>>
            + Send
            + Sync,
    >,
    pub observed_harnesses: Arc<
        dyn Fn(
                Option<String>,
            )
                -> BoxFut<Result<Vec<crate::fitness::observed::ObservedHarness>, String>>
            + Send
            + Sync,
    >,
    pub observed_models: Arc<
        dyn Fn() -> BoxFut<Result<Vec<crate::fitness::observed::ObservedModel>, String>>
            + Send
            + Sync,
    >,
    /// ISO, injected so an archive test can pin the ordering the eviction sorts
    /// on rather than racing the wall clock.
    pub now_iso: Arc<dyn Fn() -> String + Send + Sync>,
}

impl SurfaceDeps {
    /// `readSetting<T>(key, fallback)` — the edge returns raw JSON, and
    /// every typed reader funnels through here so the fallback and the error
    /// live in one place.
    pub async fn setting<T: serde::Serialize + serde::de::DeserializeOwned>(
        &self,
        key: &str,
        fallback: T,
    ) -> Result<T, String> {
        let raw = ((self.read_setting)(
            key.to_string(),
            serde_json::to_value(&fallback).map_err(|e| e.to_string())?,
        ))
        .await?;
        serde_json::from_value(raw).map_err(|e| e.to_string())
    }
}

/// The test half: every edge panics with its own name, so a store test that
/// drifts past the edges it meant to stub fails at the edge that drifted
/// rather than at a database.
pub fn panic_deps() -> SurfaceDeps {
    fn boom<T>(what: &'static str) -> BoxFut<T> {
        Box::pin(async move { panic!("surface deps edge `{what}` was touched") })
    }
    SurfaceDeps {
        models: Arc::new(|| boom("models")),
        routing: Arc::new(|_| boom("routing")),
        capabilities: Arc::new(|_| boom("capabilities")),
        forget: Arc::new(|_| boom("forget")),
        harnesses: Arc::new(|| boom("harnesses")),
        bind_slots: Arc::new(|_| boom("bindSlots")),
        read_setting: Arc::new(|_, _| boom("readSetting")),
        write_setting: Arc::new(|_, _| boom("writeSetting")),
        estimate_probes: Arc::new(|_, _| boom("estimateProbes")),
        run_probes: Arc::new(|_, _| boom("runProbes")),
        clear_eval_status: Arc::new(|_| boom("clearEvalStatus")),
        clear_transcripts: Arc::new(|_| boom("clearTranscripts")),
        mcp_servers: Arc::new(|| boom("mcpServers")),
        platform_supply: Arc::new(|| boom("platformSupply")),
        estimate_adversarial: Arc::new(|_, _| boom("estimateAdversarial")),
        run_adversarial: Arc::new(|_, _| boom("runAdversarial")),
        run_eval_sweep: Arc::new(|_, _| boom("runEvalSweep")),
        eval_sweep_statuses: Arc::new(|_| boom("evalSweepStatuses")),
        stop_eval_sweep: Arc::new(|_| panic!("surface deps edge `stopEvalSweep` was touched")),
        guard_config: Arc::new(|| boom("guardConfig")),
        reach: Arc::new(|_, _| boom("reach")),

        observed_harnesses: Arc::new(|_| boom("observedHarnesses")),
        observed_models: Arc::new(|| boom("observedModels")),
        now_iso: Arc::new(|| panic!("surface deps edge `nowIso` was touched")),
    }
}

/// The production edges. Each closure OWNS its pool/state clone (the
/// block-clone before the `move`), because a returned `BoxFut` is `'static`
/// and may not borrow from the closure that made it.
pub fn real_deps(state: &crate::state::AppState) -> SurfaceDeps {
    let pg = state.pg.clone();
    SurfaceDeps {
        models: Arc::new({
            let pg = pg.clone();
            move || {
                let pg = pg.clone();
                Box::pin(async move {
                    crate::model::access::gateway_models(&pg)
                        .await
                        .map_err(|e| e.to_string())
                })
            }
        }),
        routing: Arc::new({
            let pg = pg.clone();
            move |model| {
                let pg = pg.clone();
                Box::pin(async move {
                    crate::gateway::registry::routing_for(&pg, &model)
                        .await
                        .map_err(|e| e.to_string())
                })
            }
        }),
        capabilities: Arc::new({
            let pg = pg.clone();
            move |key| {
                let pg = pg.clone();
                Box::pin(async move { crate::capability::get_capabilities(&pg, &key).await })
            }
        }),
        forget: Arc::new({
            let pg = pg.clone();
            move |key| {
                let pg = pg.clone();
                Box::pin(async move {
                    crate::capability::forget_capabilities(&pg, &key)
                        .await
                        .map_err(|e| e.to_string())
                })
            }
        }),
        // The builtin registry is what the sweep itself binds and runs, so the
        // matrix counts and a run's own registry cannot disagree. App- and
        // workbench-authored harnesses cross with their plane, not here.
        harnesses: Arc::new(|| {
            Box::pin(
                async move { Ok(crate::harness::registry::builtin_activity_harnesses().to_vec()) },
            )
        }),
        bind_slots: Arc::new(|harnesses| {
            Box::pin(async move { Ok(crate::fitness::score::bind_slots(&harnesses).await) })
        }),
        read_setting: Arc::new({
            let pg = pg.clone();
            move |key, fallback| {
                let pg = pg.clone();
                Box::pin(async move {
                    Ok(crate::gateway::settings::get_setting(&pg, &key, fallback).await)
                })
            }
        }),
        write_setting: Arc::new({
            let pg = pg.clone();
            move |key, value| {
                let pg = pg.clone();
                Box::pin(async move {
                    crate::gateway::settings::set_setting(&pg, &key, &value)
                        .await
                        .map_err(|e| e.to_string())
                })
            }
        }),
        estimate_probes: Arc::new({
            let st = state.clone();
            move |model, reprobe| {
                let st = st.clone();
                Box::pin(async move {
                    Ok(
                        crate::fitness::probes::estimate_probes(&st, &model, None, None, reprobe)
                            .await,
                    )
                })
            }
        }),
        run_probes: Arc::new({
            let st = state.clone();
            move |model, reprobe| {
                let st = st.clone();
                Box::pin(async move {
                    crate::fitness::probes::run_probes(
                        &st,
                        &model,
                        crate::fitness::probes::ProbeOpts {
                            reprobe,
                            ..Default::default()
                        },
                    )
                    .await
                })
            }
        }),
        clear_eval_status: Arc::new({
            let pg = pg.clone();
            move |model| {
                let pg = pg.clone();
                Box::pin(async move {
                    crate::fitness::evals::clear_eval_status(&pg, &model).await;
                    Ok(())
                })
            }
        }),
        clear_transcripts: Arc::new({
            let pg = pg.clone();
            move |model| {
                let pg = pg.clone();
                Box::pin(async move {
                    crate::fitness::transcripts::clear_transcripts(&pg, model.as_deref())
                        .await
                        .map(|n| n as i64)
                        .map_err(|e| e.to_string())
                })
            }
        }),
        mcp_servers: Arc::new({
            let pg = pg.clone();
            move || {
                let pg = pg.clone();
                Box::pin(async move {
                    let reach = crate::capability_reach::DbReach { pg: &pg };
                    Ok(crate::capability_reach::ReachDeps::servers(&reach).await)
                })
            }
        }),
        platform_supply: Arc::new({
            let pg = pg.clone();
            move || {
                let pg = pg.clone();
                Box::pin(async move { Ok(crate::capability_reach::platform_supply(&pg).await) })
            }
        }),
        estimate_adversarial: Arc::new(|adversary, price| {
            Box::pin(async move {
                Ok(crate::fitness::adversarial::estimate_adversarial(
                    None,
                    adversary.as_deref(),
                    price,
                )
                .await)
            })
        }),
        run_adversarial: Arc::new({
            let st = state.clone();
            move |model, adversary| {
                let st = st.clone();
                Box::pin(async move {
                    Ok(crate::fitness::adversarial::run_adversarial(
                        &st,
                        &model,
                        crate::fitness::adversarial::AdversarialOptions {
                            adversary_model: adversary,
                            ..Default::default()
                        },
                    )
                    .await)
                })
            }
        }),
        // THE STOP READER AND THE TRANSCRIPT ARCHIVER are the surface's own —
        // exactly these three go into `run_eval_sweep`, and a sweep started
        // anywhere else would neither stop on request nor file its evidence.
        run_eval_sweep: Arc::new({
            let st = state.clone();
            move |model, start| {
                let st = st.clone();
                Box::pin(async move {
                    Ok(crate::fitness::evals::run_eval_sweep(
                        &st,
                        &model,
                        crate::fitness::evals::EvalOptions {
                            restart: start.restart,
                            only: start.only,
                            retry_failed: start.retry_failed,
                            supplement: start.supplement,
                            concurrency: start.concurrency,
                            should_stop: Some(Arc::new({
                                let st = st.clone();
                                move |m| {
                                    let st = st.clone();
                                    Box::pin(async move {
                                        stop_requested_for(&m, &real_deps(&st))
                                            .await
                                            .unwrap_or(false)
                                    })
                                }
                            })),
                            archive_case: Some(Arc::new({
                                let st = st.clone();
                                move |model, run, case| {
                                    let st = st.clone();
                                    Box::pin(async move {
                                        crate::fitness::transcripts::record_transcript(
                                            &st.pg, &model, &run, &case,
                                        )
                                        .await;
                                    })
                                }
                            })),
                            archive_prune: Some(Arc::new({
                                let st = st.clone();
                                move |model| {
                                    let st = st.clone();
                                    Box::pin(async move {
                                        let _ = crate::fitness::transcripts::prune_transcripts(
                                            &st.pg,
                                            &model,
                                            crate::fitness::transcripts::KEEP_RUNS_PER_MODEL,
                                        )
                                        .await;
                                    })
                                }
                            })),
                            ..Default::default()
                        },
                    )
                    .await)
                })
            }
        }),
        eval_sweep_statuses: Arc::new({
            let pg = pg.clone();
            move |models| {
                let pg = pg.clone();
                Box::pin(async move {
                    Ok(crate::fitness::evals::eval_sweep_statuses(&pg, &models).await)
                })
            }
        }),
        stop_eval_sweep: Arc::new(|model| crate::fitness::evals::stop_eval_sweep(model.as_deref())),
        guard_config: Arc::new({
            let pg = pg.clone();
            move || {
                let pg = pg.clone();
                Box::pin(async move { Ok(crate::gateway::guard::guard_config(&pg).await) })
            }
        }),
        reach: Arc::new({
            let pg = pg.clone();
            move |model, wanted| {
                let pg = pg.clone();
                Box::pin(async move {
                    let keys = crate::harness::run::capability_keys_for(&pg, &model).await;
                    let wanted: Vec<&str> = wanted.iter().map(|w| w.as_str()).collect();
                    Ok(crate::capability_reach::reach_for_keys(&pg, &keys, &wanted).await)
                })
            }
        }),
        observed_harnesses: Arc::new({
            let pg = pg.clone();
            move |model| {
                let pg = pg.clone();
                Box::pin(async move {
                    let deps = crate::fitness::observed::real_deps(pg.clone());
                    Ok(crate::fitness::observed::observed_harnesses(
                        &deps,
                        &crate::fitness::observed::ObservedOptions {
                            since_days: None,
                            model,
                        },
                    )
                    .await)
                })
            }
        }),
        observed_models: Arc::new({
            let pg = pg.clone();
            move || {
                let pg = pg.clone();
                Box::pin(async move {
                    let deps = crate::fitness::observed::real_deps(pg.clone());
                    Ok(crate::fitness::observed::observed_models(
                        &deps,
                        &crate::fitness::observed::ObservedOptions::default(),
                    )
                    .await)
                })
            }
        }),
        now_iso: Arc::new(now_iso),
    }
}

/// The wall clock, as `new Date().toISOString()` writes it.
pub fn now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    crate::agent_auth::epoch_ms_to_iso(now.as_millis() as i64)
}

// ── Model rows and capability facts ──────────────────────────────────────────

/// FOUR STATES, and the fourth is a real one.
///
/// `Supplied` means the MODEL cannot do it and the DEPLOYMENT can — a
/// registered tool supplies it (`capability_reach.rs`). That is neither a yes
/// nor a no and must not be collapsed into either: calling it `Yes` claims a
/// blind model sees, and calling it `No` refuses a deployment that can
/// genuinely do the job. It is the distinction `capability_reach.rs` already
/// draws with `via: native | tool`, surfaced where an admin actually picks a
/// model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityState {
    Yes,
    No,
    Unknown,
    Supplied,
}

impl CapabilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            CapabilityState::Yes => "yes",
            CapabilityState::No => "no",
            CapabilityState::Unknown => "unknown",
            CapabilityState::Supplied => "supplied",
        }
    }
}

/// One capability as the UI shows it. `state` is three-valued (plus supplied)
/// and that is the whole point: KNOWN-TRUE, KNOWN-FALSE and NEVER-MEASURED are
/// three different facts, and `missing_capabilities` only ever treats the
/// middle one as a lack. A two-valued tag would turn every unprobed model on a
/// fresh self-host into a wall of red.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FactView {
    pub state: CapabilityState,
    pub source: Option<String>,
    pub detail: Option<String>,
    pub score: Option<f64>,
    pub at: Option<String>,
    /// What supplies it, when `state` is `Supplied`. Named so the tag can say
    /// WHICH tool — "supplied" with no attribution is a claim an admin cannot
    /// check, and the supplier is the thing that might be switched off
    /// tomorrow.
    pub via: Option<crate::capability_reach::Supplier>,
}

/// [`FactView`] with its capability named — the shape a row carries.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityView {
    pub cap: String,
    #[serde(flatten)]
    pub view: FactView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub id: String,
    /// `endpoint/model` — one endpoint, one capability key, probeable.
    pub qualified: bool,
    pub endpoints: Vec<String>,
    /// A bare id served by MORE THAN ONE endpoint. Capability is a property of
    /// the endpoint, so a pooled id's facts can disagree — and `run_probes`
    /// refuses to write under a pooled key for exactly that reason. The row is
    /// shown, its facts are shown where every member agrees, and the UI points
    /// at the endpoint-qualified ids for a run.
    pub pooled: bool,
    pub capabilities: Vec<CapabilityView>,
}

/// The `endpoint:model` keys an id's facts live under, derived from the model
/// catalog rather than by asking the router per model. `gateway_models` builds
/// qualified ids as `<endpoint>/<model>`, which is the same decomposition
/// `routing_for` does — and doing it here costs one query for the whole page
/// instead of one per model.
pub fn keys_for(id: &str, qualified: bool, endpoints: &[String]) -> Vec<String> {
    // `id.slice(id.indexOf('/') + 1)`: no '/' means the whole id, which the
    // `unwrap_or` reproduces.
    let upstream = if qualified {
        id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
    } else {
        id
    };
    endpoints
        .iter()
        .map(|ep| crate::capability::capability_key(ep, upstream))
        .collect()
}

/// Display order for the capability tags, as a rank rather than a list. The
/// rank map it is sorted from is exhaustive over the union — a tenth
/// capability fails the build here instead of quietly never rendering a tag.
pub const CAPABILITY_ORDER: [&str; 9] = [
    "json",
    "json-strict",
    "tools",
    "tool-select",
    "search",
    "code",
    "long-context",
    "vision",
    "instruction-following",
];

/// The sentence a pooled id gets when its endpoints disagree. Public so the
/// test asserts the string an admin actually reads rather than a paraphrase.
pub const POOLED_DISAGREEMENT: &str =
    "The endpoints serving this model id disagree. Test the endpoint-qualified id instead.";

/// Merge one capability across every endpoint that could serve the id.
///
/// DISAGREEMENT IS UNKNOWN, NOT A VOTE. A bare id round-robins across its
/// pool, so a call lands on one member; if the vendor API can hold JSON mode
/// and the local llama.cpp build cannot, the honest answer for the pooled id
/// is "it depends", and the only safe rendering of "it depends" is
/// unmeasured. The alternative — crediting the better member — is the false
/// `true` that `run_probes` refuses to write.
///
/// A MISSING MEMBER IS ALSO UNKNOWN, in BOTH directions: a pool where one
/// endpoint says `true` and the other has never been measured is not a `yes`
/// (the unmeasured one may well fail), and a pool where one says `false` and
/// the other is unmeasured is not a `no` either — "unknown is not false" is
/// the rule the whole capability model rests on, and downgrading an
/// unmeasured member to a lack is what turns a fresh self-host into a wall of
/// red.
pub fn merge_fact(facts: &[Option<&crate::capability::CapabilityFact>]) -> FactView {
    let unknown = FactView {
        state: CapabilityState::Unknown,
        source: None,
        detail: None,
        score: None,
        at: None,
        via: None,
    };
    if facts.iter().any(|f| f.is_none()) || facts.is_empty() {
        return unknown;
    }
    let known: Vec<&crate::capability::CapabilityFact> = facts.iter().map(|f| f.unwrap()).collect();
    let first = known[0];
    if known.iter().any(|f| f.value != first.value) {
        return FactView {
            detail: Some(POOLED_DISAGREEMENT.to_string()),
            ..unknown
        };
    }
    FactView {
        state: if first.value {
            CapabilityState::Yes
        } else {
            CapabilityState::No
        },
        source: Some(first.source.clone()),
        detail: first.detail.clone(),
        score: first.score,
        at: Some(first.at.clone()),
        via: None,
    }
}

/// THE DEPLOYMENT CAN, EVEN THOUGH THE MODEL CANNOT.
///
/// Applied after `merge_fact`, and only ever to a `No` or an `Unknown`: a
/// model that does the thing natively is not "supplied", and overwriting a
/// measured `Yes` would hide the fact that no tool is needed. Everything else
/// is left exactly as measured — this promotes reach, it never invents a
/// capability.
///
/// WHY IT IS NOT A `Yes`. Calling it one claims a blind model sees, which is
/// the false-true this whole capability model is built to avoid; calling it a
/// `No` refuses a deployment that can genuinely do the job. It is a third
/// fact and it gets a third tag.
pub fn supplied_by(
    view: FactView,
    supplier: Option<crate::capability_reach::Supplier>,
) -> FactView {
    match supplier {
        Some(supplier)
            if view.state == CapabilityState::No || view.state == CapabilityState::Unknown =>
        {
            FactView {
                state: CapabilityState::Supplied,
                detail: Some(format!(
                    "the model does not do this itself; '{}.{}' supplies it",
                    supplier.server, supplier.tool
                )),
                via: Some(supplier),
                ..view
            }
        }
        _ => view,
    }
}

/// The archive, re-keyed onto the ids the catalog now offers.
///
/// A report archived under `deepseek/deepseek-v4-flash` is a report about the
/// deployment now called `openrouter/deepseek/deepseek-v4-flash` — same
/// endpoint, same capability key, same weights. Left alone, that run's
/// verdicts would light no cell and the admin would be asked to buy it again.
///
/// A canonical entry always WINS over a bare one that maps onto it: if both
/// exist, the qualified id was tested more recently or more specifically, and
/// in either case it is the one that named its endpoint.
pub fn canonical_index(
    index: &FitnessIndex,
    catalog: &[crate::model::access::GatewayModel],
) -> FitnessIndex {
    let mut out: FitnessIndex = FitnessIndex::new();
    for (model, entry) in index {
        let id = crate::model::access::canonical_model_id(model, catalog);
        // `entry.model` KEEPS THE STORED SPELLING while the KEY becomes the
        // canonical one, and the distinction is load-bearing rather than tidy.
        // Re-keying the index moves where the page LOOKS a model up; it does
        // not move the archive, which still lives at `model_fitness_report:
        // <the id the run used>`. Overwriting `model` with the canonical id
        // broke every reader that goes on to fetch the report by it: the
        // drill-down found no record for a model it had just drawn a full row
        // of verdicts for, and the value view's backfill silently gave up and
        // fell back to the shared token budget. `stored_id_for` is the one way
        // back.
        if id == *model || !out.contains_key(&id) {
            out.insert(id, entry.clone());
        }
    }
    // Second pass so an entry already under its canonical id is never
    // displaced by one that merely maps there.
    for (model, entry) in index {
        if catalog.iter().any(|m| &m.id == model) {
            out.insert(model.clone(), entry.clone());
        }
    }
    out
}

/// THE ID THIS MODEL'S ARCHIVE IS FILED UNDER.
///
/// Not always the id the catalog now offers: a run archives under whatever id
/// it was started with, and `canonical_index` re-keys the index onto the
/// offered spelling without moving the report. Every reader that turns a row
/// into a `record_key` has to come through here. New runs archive under the
/// canonical id already, so this stops mattering as old reports age out.
pub fn stored_id_for(model: &str, index: &FitnessIndex) -> String {
    index
        .get(model)
        .map(|e| e.model.clone())
        .unwrap_or_else(|| model.to_string())
}

/// Every catalog row with its nine capability tags. One `capabilities` read
/// per DISTINCT `endpoint:model` key — a bare id and its qualified sibling
/// share a key, so deduping the reads is the point.
pub async fn model_rows(deps: &SurfaceDeps) -> Result<Vec<ModelRow>, String> {
    let models = ((deps.models)()).await?;
    let mut facts_cache: HashMap<String, HashMap<String, crate::capability::CapabilityFact>> =
        HashMap::new();
    let mut keys_of: Vec<(String, Vec<String>)> = Vec::with_capacity(models.len());
    for m in &models {
        let keys = keys_for(&m.id, m.qualified, &m.endpoints);
        for key in &keys {
            if !facts_cache.contains_key(key) {
                // A read that fails is no facts, not no model.
                let facts = ((deps.capabilities)(key.clone())).await;
                facts_cache.insert(key.clone(), facts);
            }
        }
        keys_of.push((m.id.clone(), keys));
    }

    // WHAT THE DEPLOYMENT SUPPLIES, read once for the whole list. A registered
    // tool is a property of the install, not of a model, so asking per row
    // would be one listing per model for an answer that cannot differ between
    // them.
    let servers = ((deps.mcp_servers)()).await.unwrap_or_default();
    let providers: crate::capability_reach::Providers = deps
        .setting(crate::capability_reach::PROVIDERS_KEY, HashMap::new())
        .await
        .unwrap_or_default();
    // Talaria's own checked tools, under the registry — so a `supplied` tag
    // shows on an install that has registered nothing but can still do the
    // work.
    let platform = ((deps.platform_supply)()).await.unwrap_or_default();
    let suppliers: HashMap<&str, Option<crate::capability_reach::Supplier>> = CAPABILITY_ORDER
        .iter()
        .map(|cap| {
            (
                *cap,
                crate::capability_reach::supplier_for(cap, &servers, &providers, &platform),
            )
        })
        .collect();

    let mut rows: Vec<ModelRow> = Vec::with_capacity(models.len());
    for (i, m) in models.iter().enumerate() {
        let keys = &keys_of[i].1;
        let per_key: Vec<&HashMap<String, crate::capability::CapabilityFact>> =
            keys.iter().map(|k| &facts_cache[k]).collect();
        rows.push(ModelRow {
            id: m.id.clone(),
            qualified: m.qualified,
            endpoints: m.endpoints.clone(),
            pooled: !m.qualified && m.endpoints.len() > 1,
            capabilities: CAPABILITY_ORDER
                .iter()
                .map(|cap| {
                    let facts: Vec<Option<&crate::capability::CapabilityFact>> =
                        per_key.iter().map(|f| f.get(*cap)).collect();
                    CapabilityView {
                        cap: cap.to_string(),
                        view: supplied_by(
                            merge_fact(&facts),
                            suppliers.get(cap).cloned().flatten(),
                        ),
                    }
                })
                .collect(),
        });
    }
    Ok(rows)
}

/// Facts merged across every key the probe run wrote under, for scoring. A run
/// against a pooled id writes nothing (the ambiguity rule in `run_probes`), so
/// in practice this is one key.
pub fn capabilities_of(
    row: Option<&ModelRow>,
) -> HashMap<String, crate::capability::CapabilityFact> {
    let mut out = HashMap::new();
    let Some(row) = row else { return out };
    for view in &row.capabilities {
        // Only a settled fact reaches the scorer. `Unknown` must arrive as
        // ABSENT, because score.rs distinguishes "recorded false" (unfit) from
        // "never measured" (workable, run the probes) and a synthesized
        // `false` would collapse the two into the harsher one.
        let (Some(source), Some(at)) = (view.view.source.clone(), view.view.at.clone()) else {
            continue;
        };
        if view.view.state == CapabilityState::Unknown {
            continue;
        }
        let mut fact = crate::capability::CapabilityFact {
            value: view.view.state == CapabilityState::Yes,
            source,
            at,
            detail: None,
            score: None,
        };
        if let Some(d) = &view.view.detail {
            fact.detail = Some(d.clone());
        }
        if let Some(s) = view.view.score {
            fact.score = Some(s);
        }
        out.insert(view.cap.clone(), fact);
    }
    out
}

// ── Pricing ──────────────────────────────────────────────────────────────────

/// $/MTok for the DEAREST endpoint that could serve this model.
///
/// Dearest rather than average for the reason `fitness/probes.rs` gives for
/// the identical derivation: an estimate the round-robin can exceed is not an
/// estimate an admin can act on. That module keeps its copy private, so this
/// is the second one — exporting it belongs to that file rather than here,
/// and is the fix if a third ever appears.
pub async fn price_of(model: &str, deps: &SurfaceDeps) -> Result<Option<ModelPrice>, String> {
    // A routing failure is no price, not no estimate: the tier still reports
    // its tokens and call count, which are the numbers that do not depend on a
    // catalog being reachable.
    let Ok(route) = ((deps.routing)(model.to_string())).await else {
        return Ok(None);
    };
    if route.endpoints.is_empty() {
        return Ok(None);
    }
    let at = |map: &Value, upstream: &str| -> Option<f64> {
        map.get(upstream)?.as_object()?.get("in")?.as_f64()
    };
    let out = |map: &Value, upstream: &str| -> Option<f64> {
        map.get(upstream)?.as_object()?.get("out")?.as_f64()
    };
    let priced: Vec<ModelPrice> = route
        .endpoints
        .iter()
        .filter_map(|ep| {
            let in_tok = at(&ep.model_prices, &route.upstream_model)
                .or_else(|| at(&ep.auto_prices, &route.upstream_model))
                .or(ep.price_in_per_mtok);
            let out_tok = out(&ep.model_prices, &route.upstream_model)
                .or_else(|| out(&ep.auto_prices, &route.upstream_model))
                .or(ep.price_out_per_mtok);
            Some(ModelPrice {
                in_per_mtok: in_tok?,
                out_per_mtok: out_tok?,
            })
        })
        .collect();
    Ok(priced.into_iter().max_by(|a, b| {
        (a.in_per_mtok + a.out_per_mtok)
            .partial_cmp(&(b.in_per_mtok + b.out_per_mtok))
            .unwrap_or(std::cmp::Ordering::Equal)
    }))
}

pub fn usd_of(
    price: Option<ModelPrice>,
    prompt_tokens: i64,
    completion_tokens: i64,
) -> Option<f64> {
    price.map(|p| {
        (prompt_tokens as f64 * p.in_per_mtok + completion_tokens as f64 * p.out_per_mtok) / 1e6
    })
}

// ── The estimate ─────────────────────────────────────────────────────────────

/// What the tokens ARE, because they are not the same kind of number in every
/// tier and an admin comparing them deserves to know which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EstimateBasis {
    Fixture,
    Measured,
    Ceiling,
}

impl EstimateBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            EstimateBasis::Fixture => "fixture",
            EstimateBasis::Measured => "measured",
            EstimateBasis::Ceiling => "ceiling",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierEstimate {
    pub tier: TierId,
    pub calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub usd: Option<f64>,
    pub basis: EstimateBasis,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEstimate {
    pub model: String,
    pub adversary_model: Option<String>,
    pub tiers: Vec<TierEstimate>,
    pub calls: i64,
    /// None when nothing prices this model. The call count is still exact, and
    /// it is the number that does not depend on a catalog being reachable.
    pub usd: Option<f64>,
    pub priced: bool,
    /// Harnesses whose per-case tokens nothing has measured yet. Non-zero means
    /// the tier-2 dollar figure is a FLOOR, and the UI says so.
    pub unmeasured_harnesses: i64,
    pub fixtures: i64,
}

#[derive(Clone)]
pub struct Tier2Shape {
    pub harnesses: Vec<crate::harness::registry::RegisteredHarness>,
    pub fixtures: i64,
    /// One repair turn per JSON fixture, worst case. The runner sends a repair
    /// only when the contract fails, so this is a ceiling and never a surprise.
    pub repair_ceiling: i64,
}

pub async fn tier2_shape(
    only: Option<&[String]>,
    deps: &SurfaceDeps,
) -> Result<Tier2Shape, String> {
    let all = ((deps.harnesses)()).await?;
    let harnesses: Vec<_> = match only {
        Some(only) if !only.is_empty() => all
            .into_iter()
            .filter(|h| only.contains(&h.def.id.to_string()))
            .collect(),
        _ => all,
    };
    let mut fixtures = 0i64;
    let mut repair_ceiling = 0i64;
    for h in &harnesses {
        fixtures += h.eval_names().count() as i64;
        // Mirrors `meta_of` in evals.rs, which mirrors the runner's repair
        // rule: a text harness never gets a repair turn, so budgeting one for
        // it would inflate every estimate on a registry that is
        // thirteen-fourteenths text.
        if crate::fitness::evals::meta_of(h).repairable {
            repair_ceiling += h.eval_names().count() as i64;
        }
    }
    Ok(Tier2Shape {
        harnesses,
        fixtures,
        repair_ceiling,
    })
}

#[derive(Debug, Clone)]
pub struct EstimateRequest {
    /// Mirrors `StartOptions.reprobe`, so the price shown is the price of the
    /// run the button will actually start.
    pub reprobe: bool,
    pub model: String,
    pub tiers: Vec<TierId>,
    pub adversary_model: Option<String>,
    pub only: Option<Vec<String>>,
}

pub async fn estimate_run(
    req: &EstimateRequest,
    deps: &SurfaceDeps,
) -> Result<RunEstimate, String> {
    let price = price_of(&req.model, deps).await?;
    let mut rows: Vec<TierEstimate> = Vec::new();
    let shape = tier2_shape(req.only.as_deref(), deps).await?;
    let mut unmeasured = 0i64;

    if req.tiers.contains(&TierId::Probes) {
        // A probe that will skip costs nothing and `estimate_probes` already
        // zeroes its calls; this only sums what it hands back, so a skipped
        // vision probe subtracts from the total here without a second copy of
        // the skip rule.
        let est = ((deps.estimate_probes)(req.model.clone(), req.reprobe))
            .await
            .ok();
        let est = est.as_ref();
        rows.push(TierEstimate {
            tier: TierId::Probes,
            calls: est.map(|e| e.calls).unwrap_or(0),
            prompt_tokens: est.map(|e| e.prompt_tokens).unwrap_or(0),
            completion_tokens: est.map(|e| e.completion_tokens).unwrap_or(0),
            usd: est.and_then(|e| e.usd),
            basis: EstimateBasis::Fixture,
            note: if est.map(|e| e.known).unwrap_or(0) > 0 {
                format!(
                    "{} capability(ies) were already measured on this endpoint and are reused, not re-bought. Tick \"re-measure capabilities\" to pay for them again.",
                    est.map(|e| e.known).unwrap_or(0)
                )
            } else {
                "Fixed prompts, so this is exact, except the long-context probe, which is sized from the model’s own advertised window.".to_string()
            },
        });
    }

    if req.tiers.contains(&TierId::Evals) {
        let budget: TokenBudget = deps.setting(BUDGET_KEY, TokenBudget::new()).await?;
        let mut prompt_tokens = 0i64;
        let mut completion_tokens = 0i64;
        for h in &shape.harnesses {
            let n = h.eval_names().count() as i64;
            if n == 0 {
                continue;
            }
            let Some(b) = budget.get(h.def.id) else {
                unmeasured += 1;
                continue;
            };
            prompt_tokens += b.prompt * n;
            completion_tokens += b.completion * n;
        }
        rows.push(TierEstimate {
            tier: TierId::Evals,
            calls: shape.fixtures + shape.repair_ceiling,
            prompt_tokens,
            completion_tokens,
            usd: usd_of(price, prompt_tokens, completion_tokens),
            basis: EstimateBasis::Measured,
            note: if unmeasured == 0 {
                "Tokens are what these fixtures actually cost the last time each harness ran.".to_string()
            } else {
                format!(
                    "Tokens are what these fixtures cost the last time each harness ran. {unmeasured} harness(es) have never run, so the figure is a floor."
                )
            },
        });
    }

    if req.tiers.contains(&TierId::Adversarial) {
        // The adversary is a different and usually dearer model, and the run
        // pays for both. Pricing every token at the dearer of the two keeps the
        // number a ceiling rather than a pleasant surprise in the wrong
        // direction.
        let adversary_price = match &req.adversary_model {
            Some(m) => price_of(m, deps).await?,
            None => None,
        };
        let worst = match (price, adversary_price) {
            (Some(p), Some(a)) => Some(
                if p.in_per_mtok + p.out_per_mtok >= a.in_per_mtok + a.out_per_mtok {
                    p
                } else {
                    a
                },
            ),
            (None, a) => a,
            (p, None) => p,
        };
        let price_fn: crate::fitness::adversarial::PriceFn =
            Arc::new(move |p, c| Box::pin(async move { usd_of(worst, p, c) }));
        let est = ((deps.estimate_adversarial)(req.adversary_model.clone(), Some(price_fn)))
            .await
            .ok();
        let est = est.as_ref();
        rows.push(TierEstimate {
            tier: TierId::Adversarial,
            calls: est.map(|e| e.calls as i64).unwrap_or(0)
                + est.map(|e| e.adversary_calls as i64).unwrap_or(0),
            prompt_tokens: est.map(|e| e.prompt_tokens).unwrap_or(0),
            completion_tokens: est.map(|e| e.completion_tokens).unwrap_or(0),
            usd: est.and_then(|e| e.cost_usd),
            basis: EstimateBasis::Ceiling,
            note: if req.adversary_model.is_some() {
                "A ceiling: the escalation round only runs on seeds the model survives, and this assumes it survives all of them. Adversary calls are priced at the dearer of the two models.".to_string()
            } else {
                "The seed corpus only. Naming an adversary adds an escalation round.".to_string()
            },
        });
    }

    let usd_rows: Vec<f64> = rows.iter().filter_map(|r| r.usd).collect();
    let calls: i64 = rows.iter().map(|r| r.calls).sum();
    // None unless EVERY requested tier priced. A partial total under a dollar
    // sign is a number nobody can reconcile with the invoice.
    let usd = (!rows.is_empty() && usd_rows.len() == rows.len()).then(|| usd_rows.iter().sum());
    Ok(RunEstimate {
        model: req.model.clone(),
        adversary_model: req.adversary_model.clone(),
        tiers: rows,
        calls,
        usd,
        priced: price.is_some(),
        unmeasured_harnesses: unmeasured,
        fixtures: shape.fixtures,
    })
}

// ── The run ──────────────────────────────────────────────────────────────────

/// The band thresholds, sent rather than restated in a Svelte file. A cell
/// tooltip prints "contract 91%, Ready needs 95%", and a second copy of 0.95
/// in the client is how that sentence and score.rs come to disagree.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thresholds {
    pub contract_ready: f64,
    pub contract_unfit: f64,
    pub repair_workable: f64,
    pub observed_window_days: i64,
    pub min_observed_runs: i64,
}

pub fn thresholds() -> Thresholds {
    Thresholds {
        contract_ready: crate::fitness::score::CONTRACT_READY,
        contract_unfit: crate::fitness::score::CONTRACT_UNFIT,
        repair_workable: crate::fitness::score::REPAIR_WORKABLE,
        observed_window_days: crate::fitness::observed::DEFAULT_WINDOW_DAYS,
        min_observed_runs: crate::fitness::observed::MIN_OBSERVED_RUNS,
    }
}

/// THE MATRIX'S COLUMNS — the slots a run can actually say something about.
///
/// A SLOT NOTHING REACHES IS NOT A COLUMN. Several slots have no harness bound
/// to them and never will as things stand, so every cell in them reads
/// `unbound` for every model, forever. Dead columns in a table an admin
/// already has to scroll sideways is not caution, it is noise — and worse, it
/// inflates the "untested" count in every row summary with slots that were
/// never testable.
///
/// NOTHING STOPS BEING MEASURED. `fitness_slots()` is unchanged, so
/// `score_fitness` still produces a verdict for all of them and the archive
/// still stores them; `role_assignment_issues` still warns an admin who
/// assigns a blind model to the reserved vision role, which is the surface
/// where that warning belongs. This decides what gets a column, and nothing
/// else.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    #[serde(flatten)]
    pub slot: crate::fitness::score::FitnessSlot,
    // camelCase for `taskFloor`, which the panel reads off every slot row.
    pub key: String,
    pub task_floor: f64,
}

pub fn slot_views() -> Vec<SlotView> {
    crate::fitness::score::fitness_slots()
        .into_iter()
        .filter(|s| s.live)
        .map(|s| SlotView {
            key: crate::fitness::score::slot_key(s.kind, &s.id),
            task_floor: crate::fitness::score::task_floor_for(&s, None),
            slot: s,
        })
        .collect()
}

pub struct IndexEntryParts<'a> {
    pub model: &'a str,
    pub at: &'a str,
    pub ran: &'a [TierId],
    pub requested: &'a [TierId],
    pub sweep: &'a crate::fitness::evals::EvalSweep,
    pub report: &'a crate::fitness::score::FitnessReport,
    pub probes: Option<&'a crate::fitness::probes::ProbeReport>,
    pub adversarial: Option<&'a crate::fitness::adversarial::AdversarialReport>,
    /// The reading the last run left behind. Carried so a pass that measured
    /// NOTHING — a probes-only run, a sweep the admin stopped at case one —
    /// keeps the previous number instead of blanking a column somebody paid
    /// for.
    pub previous_speed: Option<SpeedReading>,
}

/// The matrix row, assembled from what the run produced.
pub fn index_entry_of(parts: IndexEntryParts<'_>) -> FitnessIndexEntry {
    let mut cells: HashMap<String, FitnessCell> = HashMap::new();
    for slot in &parts.report.slots {
        cells.insert(
            crate::fitness::score::slot_key(slot.slot.kind, &slot.slot.id),
            FitnessCell {
                band: slot.band,
                reason: slot.reasons.first().map(|r| r.detail.clone()),
            },
        );
    }
    // NULL UNLESS EVERY COMPONENT THAT SPENT ANYTHING PRICED. A partial total
    // under a dollar sign is a number nobody can reconcile with the invoice,
    // which is worse than no number — that half is unchanged.
    //
    // WHAT COUNTS AS A COMPONENT. A harness whose cases were all SKIPPED
    // reports `cost_usd: null` because it priced nothing — and it spent
    // nothing, so it is free rather than unpriced. Treating those two the same
    // meant one skipped harness turned a fully-priced run into "unpriced" in
    // the modal header. A harness that burned tokens and could not be priced
    // still poisons the total, which is the case the rule was written for.
    let spent = |cost_usd: Option<f64>, prompt: i64, completion: i64| {
        cost_usd.is_some() || prompt > 0 || completion > 0
    };
    let mut billed: Vec<Option<f64>> = parts
        .sweep
        .harnesses
        .iter()
        .filter(|h| spent(h.cost_usd, h.prompt_tokens, h.completion_tokens))
        .map(|h| h.cost_usd)
        .collect();
    if let Some(a) = parts.adversarial
        && spent(a.cost_usd, a.prompt_tokens, a.completion_tokens)
    {
        billed.push(a.cost_usd);
    }
    let cost_usd = if billed.is_empty() || billed.iter().any(|c| c.is_none()) {
        None
    } else {
        Some(billed.iter().filter_map(|c| *c).sum())
    };

    FitnessIndexEntry {
        model: parts.model.to_string(),
        at: parts.at.to_string(),
        tiers: parts.ran.to_vec(),
        guarded: parts.sweep.guarded,
        cells,
        safety: parts.adversarial.map(|a| SafetyCell {
            band: a.band,
            resistance: a.resistance,
        }),
        probes_wrote: parts.probes.map(|p| p.wrote).unwrap_or(0),
        // FROM THE PASS THAT JUST RAN. A supplemental or speed-only pass
        // refreshes the reading without re-buying the battery; a pass that ran
        // nothing (probes only) leaves the previous reading alone rather than
        // nulling it.
        speed: speed_of(&parts.sweep.measured, parts.sweep.concurrency.ended)
            .or(parts.previous_speed),
        cost_usd,
        calls: parts.sweep.done
            + parts.adversarial.map(|a| a.cases.len() as i64).unwrap_or(0)
            + parts.probes.map(|p| p.results.len() as i64).unwrap_or(0),
        // Partial in either of the two ways a run can be: tier 2 stopped
        // mid-sweep, or a tier the admin asked for never produced a result.
        partial: parts.sweep.state == crate::fitness::evals::EvalSweepState::Stopped
            || (parts.sweep.total > 0 && parts.sweep.done < parts.sweep.total)
            || parts.ran.len() < parts.requested.len(),
        // The per-harness half, from the one definition the backfill also uses.
        harnesses: Some(crate::fitness::value::harness_summary(
            parts.report,
            &parts.sweep.harnesses,
        )),
    }
}

/// The budget a sweep leaves behind, in one place. A failure to write is
/// swallowed: the next sweep re-derives the budget from its own run, and a
/// failed archive write must not void a run the org already paid for.
async fn record_budget(harnesses: &[crate::fitness::evals::HarnessScore], deps: &SurfaceDeps) {
    if harnesses.is_empty() {
        return;
    }
    let Ok(prev) = deps.setting(BUDGET_KEY, TokenBudget::new()).await else {
        return;
    };
    let next = next_budget(&prev, harnesses, &(deps.now_iso)());
    if let Ok(v) = serde_json::to_value(&next) {
        let _ = ((deps.write_setting)(BUDGET_KEY.to_string(), v)).await;
    }
}

/// The candidates running IN THIS PROCESS, each with its own stop flag. Stop is
/// honored BETWEEN tiers as well as inside tier 2 — `stop_eval_sweep` only
/// reaches the sweep, and a run stopped during the probes would otherwise go on
/// to buy the whole tier-2 sweep the admin just asked it not to.
fn runs_map() -> &'static Mutex<HashMap<String, bool>> {
    static RUNS: LazyLock<Mutex<HashMap<String, bool>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    &RUNS
}

pub fn running_models() -> Vec<String> {
    runs_map().lock().unwrap().keys().cloned().collect()
}

/// Why a run cannot start. Separated from the claim so the route can say WHICH
/// of the two refusals it hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunRefusal {
    AlreadyRunning,
    AtCapacity,
}

impl RunRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            RunRefusal::AlreadyRunning => "already-running",
            RunRefusal::AtCapacity => "at-capacity",
        }
    }
}

impl std::fmt::Display for RunRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Claim a run slot SYNCHRONOUSLY. Two simultaneous Start presses both clear
/// an `if (running)` written above an `await` — the check and the claim have to
/// be one step, and the caller owns releasing it (the tail of `run_fitness`).
fn claim_run(model: &str) -> Option<RunRefusal> {
    let mut runs = runs_map().lock().unwrap();
    if runs.contains_key(model) {
        return Some(RunRefusal::AlreadyRunning);
    }
    if runs.len() >= MAX_CONCURRENT_RUNS {
        return Some(RunRefusal::AtCapacity);
    }
    runs.insert(model.to_string(), false);
    None
}

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub model: String,
    pub tiers: Vec<TierId>,
    pub adversary_model: Option<String>,
    pub only: Option<Vec<String>>,
    pub restart: bool,
    /// Re-measure capabilities we already have a probe fact for. Off by
    /// default — see `run_probes`. The release valve for a model id that was
    /// re-pointed at different weights is "Forget recorded capabilities"; this
    /// is the softer one for an admin who just wants a fresh reading.
    pub reprobe: bool,
    /// Cases in flight at once. See `DEFAULT_CONCURRENCY` in evals.rs — and
    /// note it multiplies with `MAX_CONCURRENT_RUNS`.
    pub concurrency: Option<usize>,
    /// Keep the passes, re-ask everything else. See `EvalOptions.retry_failed`.
    pub retry_failed: bool,
    /// Run only fixtures that have never been run. See
    /// `EvalOptions.supplement`.
    pub supplement: bool,
}

/// HOW LONG A RUN MAY GO QUIET before we call it dead.
///
/// DELIBERATELY LONGER THAN A PHASE, because a phase is the heartbeat's
/// interval: `write_status` fires at tier boundaries, so a single adversarial
/// pass over a slow model is one long silence with nothing wrong.
///
/// The two errors are not symmetric. Declaring a live run dead loses work
/// somebody is paying for and reports a sweep as failed while it is still
/// spending; leaving a dead one another half hour costs one stale line on a
/// panel that self-heals. So this errs long.
pub const RUN_STALE_MS: i64 = 45 * 60_000;

/// Has this run stopped breathing? Only ever asked of a `running` row.
///
/// A HEARTBEAT RATHER THAN "is it in our process map", which was the tempting
/// cheaper test. That map is empty in any instance that did not start the run —
/// the same fact `stop_fitness_run` is built around — so keying on it would
/// have one instance quietly declaring another's live sweep dead. A heartbeat
/// is true or false regardless of who is asking.
pub fn stale_run(r: &FitnessRunStatus, now: i64) -> bool {
    if r.state != FitnessRunState::Running {
        return false;
    }
    let beat = r
        .heartbeat_at
        .as_deref()
        .or(r.started_at.as_deref())
        .and_then(crate::agent_auth::iso_to_epoch_ms);
    match beat {
        Some(beat) => now - beat > RUN_STALE_MS,
        // An unparseable (or absent) stamp is not stale: the parse yields
        // nothing, and nothing compares greater. A row that cannot say when
        // it last breathed is suspicious, but "not running" is the one verdict
        // this function must not reach by guessing.
        None => false,
    }
}

/// Serializes status writes: three
/// runs write their phase transitions concurrently and `set_setting` upserts
/// the whole row — the last writer of a tick would otherwise drop its siblings'
/// progress. Tokio (not std) because the guard is held across awaited settings
/// reads and writes.
fn status_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

/// One run's status into the map. Errors are swallowed inside — a status the
/// run could not persist must not void the run.
async fn write_run_status(status: &FitnessRunStatus, deps: &SurfaceDeps) {
    let Some(model) = status.model.clone() else {
        return;
    };
    let _guard = status_lock().lock().await;
    let Ok(mut runs) = deps
        .setting::<HashMap<String, FitnessRunStatus>>(RUNS_KEY, HashMap::new())
        .await
    else {
        return;
    };
    runs.insert(model, status.clone());
    if let Ok(v) = serde_json::to_value(&runs) {
        let _ = ((deps.write_setting)(RUNS_KEY.to_string(), v)).await;
    }
}

/// EVERY WRITE IS ALSO A HEARTBEAT. Threading it through the one helper the
/// run already uses means a new phase, a finish and an error all refresh it
/// without any caller remembering to — and a second mechanism for "is this
/// alive" is a second set of stuck-state bugs, which is the note this file
/// already makes about progress counters.
async fn write_status(status: FitnessRunStatus, deps: &SurfaceDeps) {
    let mut s = status;
    s.heartbeat_at = Some((deps.now_iso)());
    write_run_status(&s, deps).await;
}

async fn set_phase(
    model: &str,
    tiers: &[TierId],
    phase: Option<&str>,
    started_at: &str,
    deps: &SurfaceDeps,
) {
    write_status(
        FitnessRunStatus {
            state: FitnessRunState::Running,
            model: Some(model.to_string()),
            tiers: tiers.to_vec(),
            phase: phase.map(str::to_string),
            started_at: Some(started_at.to_string()),
            finished_at: None,
            error: None,
            heartbeat_at: None,
        },
        deps,
    )
    .await;
}

/// Both halves of "was this run asked to stop": the in-process flag for a run
/// this instance holds, and the persisted request for everything else.
/// Checked between tiers, which is where `run_fitness` honors a stop.
async fn stopped(model: &str, deps: &SurfaceDeps) -> bool {
    if runs_map()
        .lock()
        .unwrap()
        .get(model)
        .copied()
        .unwrap_or(false)
    {
        return true;
    }
    stop_requested_for(model, deps).await.unwrap_or(false)
}

async fn read_runs(deps: &SurfaceDeps) -> Result<HashMap<String, FitnessRunStatus>, String> {
    let runs = deps
        .setting::<HashMap<String, FitnessRunStatus>>(RUNS_KEY, HashMap::new())
        .await?;
    let legacy = deps
        .setting::<Option<FitnessRunStatus>>(STATUS_KEY, None)
        .await?;
    // THE LEGACY ROW IS FOLDED IN, NOT MIGRATED. It is the status of whatever
    // run was in flight across the change to concurrent runs, and the
    // alternative to reading it is a run the admin can watch start and then
    // never see finish. It loses to a real entry for the same model, and
    // nothing writes it again.
    let mut out = runs;
    if let Some(legacy) = legacy
        && let Some(model) = &legacy.model
        && !out.contains_key(model)
    {
        out.insert(model.clone(), legacy);
    }
    Ok(out)
}

/// Has anyone asked this candidate to stop? Cached briefly because the sweep
/// asks between every case and the answer changes about once a run.
///
/// THE CLOCK HERE IS REAL epoch ms, not the injected `now_iso` — the wall
/// clock for the cache, `now_iso` for everything stored — and keeping that
/// split means a test that pins `nowIso` to fake ordering cannot
/// accidentally pin a live 2-second cache for the whole test.
fn stop_cache() -> &'static Mutex<(i64, std::collections::HashSet<String>)> {
    static CACHE: LazyLock<Mutex<(i64, std::collections::HashSet<String>)>> =
        LazyLock::new(|| Mutex::new((0, std::collections::HashSet::new())));
    &CACHE
}

const STOP_TTL_MS: i64 = 2_000;

pub async fn stop_requested_for(model: &str, deps: &SurfaceDeps) -> Result<bool, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let fresh = {
        let cache = stop_cache().lock().unwrap();
        now - cache.0 <= STOP_TTL_MS && cache.0 != 0
    };
    if !fresh {
        let asked = deps
            .setting::<Vec<String>>(STOP_KEY, Vec::new())
            .await
            .unwrap_or_default();
        *stop_cache().lock().unwrap() = (now, asked.into_iter().collect());
    }
    Ok(stop_cache().lock().unwrap().1.contains(model))
}

/// Clear the request once the run has actually ended, so the next Start is not
/// stopped by a flag left over from the last one.
pub async fn clear_stop_request(model: &str, deps: &SurfaceDeps) {
    let Ok(asked) = deps.setting::<Vec<String>>(STOP_KEY, Vec::new()).await else {
        return;
    };
    if !asked.iter().any(|m| m == model) {
        return;
    }
    let rest: Vec<String> = asked.into_iter().filter(|m| m != model).collect();
    if let Ok(v) = serde_json::to_value(&rest) {
        let _ = ((deps.write_setting)(STOP_KEY.to_string(), v)).await;
    }
    *stop_cache().lock().unwrap() = (0, std::collections::HashSet::new());
}

/// Run the requested tiers against one candidate, score, archive.
///
/// DETACHED, like `reindex_all`: the tiers are minutes of model calls and the
/// admin watches the status row. Every tier is individually caught — a probe
/// suite that cannot reach the gateway must not void a tier-2 sweep the org
/// already paid for.
///
/// THE CALLER HAS ALREADY CLAIMED the run slot (`claim_run`); this function
/// releases it. Claiming here instead would put an await between the check and
/// the claim in the route, which is a second concurrent run.
pub async fn run_fitness(opts: StartOptions, deps: Arc<SurfaceDeps>) {
    let model = opts.model.clone();
    let tiers = opts.tiers.clone();
    let started_at = (deps.now_iso)();
    // The tiers that actually PRODUCED SOMETHING, which is not the tiers that
    // were asked for once Stop — or a dead gateway — is in play. The archived
    // record is stamped with this one: a record claiming a tier that never
    // happened is the same lie as a green cell nobody filled, and a tier whose
    // runner threw did not happen even though it was attempted.
    let mut ran: Vec<TierId> = Vec::new();

    // THE CONSOLE BELONGS TO A RUN, and is cleared when one STARTS rather than
    // when one ends. Clearing at the end wiped the terminal the moment the
    // sweep finished — the point at which somebody who was watching it wants to
    // read it back.
    crate::fitness::live_feed::start_live_feed(&model);

    // The inner body takes `?` on every edge, so a failure lands in the error
    // status, never a panic and never a dropped slot.
    let outcome: Result<(), String> = async {
        set_phase(
            &model,
            &tiers,
            tiers.first().map(|t| t.as_str()).or(Some("scoring")),
            &started_at,
            &deps,
        )
        .await;

        let mut probes: Option<crate::fitness::probes::ProbeReport> = None;
        if tiers.contains(&TierId::Probes) && !stopped(&model, &deps).await {
            set_phase(&model, &tiers, Some("probes"), &started_at, &deps).await;
            probes = ((deps.run_probes)(model.clone(), opts.reprobe)).await.ok();
            if probes.is_some() {
                ran.push(TierId::Probes);
            }
        }

        let harnesses = ((deps.harnesses)()).await?;
        let mut sweep: Option<crate::fitness::evals::EvalSweep> = None;
        if tiers.contains(&TierId::Evals) && !stopped(&model, &deps).await {
            set_phase(&model, &tiers, Some("evals"), &started_at, &deps).await;
            sweep = ((deps.run_eval_sweep)(
                model.clone(),
                SweepStart {
                    restart: opts.restart,
                    only: opts.only.clone().filter(|o| !o.is_empty()),
                    retry_failed: opts.retry_failed,
                    supplement: opts.supplement,
                    concurrency: opts.concurrency,
                },
            ))
            .await
            .ok();
            if let Some(s) = &sweep {
                record_budget(&s.harnesses, &deps).await;
                ran.push(TierId::Evals);
            }
        }

        let mut adversarial: Option<crate::fitness::adversarial::AdversarialReport> = None;
        if tiers.contains(&TierId::Adversarial) && !stopped(&model, &deps).await {
            set_phase(&model, &tiers, Some("adversarial"), &started_at, &deps).await;
            adversarial = ((deps.run_adversarial)(model.clone(), opts.adversary_model.clone()))
                .await
                .ok();
            if adversarial.is_some() {
                ran.push(TierId::Adversarial);
            }
        }

        // NOTHING RAN: archive nothing. Overwriting a real verdict from last
        // week with an empty one — every cell reset to Untested — would make
        // Stop destructive, and Stop is the button an admin reaches for when a
        // run is costing more than they expected. The same rule saves the
        // archive when every tier threw.
        if ran.is_empty() {
            write_status(
                FitnessRunStatus {
                    state: FitnessRunState::Done,
                    model: Some(model.clone()),
                    tiers: tiers.clone(),
                    phase: None,
                    started_at: Some(started_at.clone()),
                    finished_at: Some((deps.now_iso)()),
                    error: None,
                    heartbeat_at: None,
                },
                &deps,
            )
            .await;
            return Ok(());
        }

        set_phase(&model, &tiers, Some("scoring"), &started_at, &deps).await;
        let unfixtured: Vec<String> = harnesses
            .iter()
            .filter(|h| h.eval_names().next().is_none())
            .map(|h| h.def.id.to_string())
            .collect();
        // With the guard off every guard rate is zero, and zero-because-off
        // must not read as zero-because-clean — score.rs caps such a run at
        // `workable` and says why. A run that skipped tier 2 has no sweep to
        // ask, so the install's own mode is read rather than a `false` stood in
        // for it, which would report the guard as off on an install where it is
        // on.
        let guarded = ((deps.guard_config)())
            .await
            .map(|c| c.mode != crate::gateway::guard::GuardMode::Off)
            .unwrap_or(false);
        // A TIER THAT DID NOT RUN KEEPS ITS LAST RESULT. IT DOES NOT GET
        // BLANKED. Re-running ONE tier is the normal thing to want — a fixture
        // changed, the provocation corpus grew, a capability needs re-measuring
        // — so the merge is the behaviour, not an option. `tier_at` records
        // when each part was last measured, because a report that carries a
        // month-old probe result must be able to say so rather than showing
        // today's date over all of it.
        let index_for_prior = read_index(&deps).await?;
        let prior = upgrade_record(
            deps.setting::<Option<FitnessRecord>>(
                &record_key(&stored_id_for(&model, &index_for_prior)),
                None,
            )
            .await
            .ok()
            .flatten(),
        );
        let carried_sweep = prior.as_ref().filter(|p| !p.harnesses.is_empty()).map(|p| {
            let mut s = empty_sweep(&model, p.sweep.unfixtured.clone(), p.report.guarded, &p.at);
            // The summary kept the state as a string. It round-trips through
            // the serde shape it was written in, and anything unparseable is
            // `idle` — which for the one comparison this feeds
            // (`state === 'stopped'`) is the safe answer.
            s.state = serde_json::from_value(serde_json::Value::String(p.sweep.state.clone()))
                .unwrap_or(crate::fitness::evals::EvalSweepState::Idle);
            s.done = p.sweep.done;
            s.total = p.sweep.total;
            s.harnesses = p.harnesses.clone();
            s.cases = p.cases.clone();
            s.concurrency = p.sweep.concurrency.clone();
            s
        });
        let effective = sweep
            .or(carried_sweep)
            .unwrap_or_else(|| empty_sweep(&model, unfixtured.clone(), guarded, &started_at));
        let observed = ((deps.observed_harnesses)(None)).await.unwrap_or_default();
        let rows = model_rows(&deps).await.unwrap_or_default();
        // WHAT THE DEPLOYMENT REACHES, asked once for every capability any
        // bound harness or slot requires. Without it a slot verdict is a
        // statement about the model alone, and the thing an admin assigns is a
        // model running inside Talaria with the tools this org registered. A
        // failure here degrades to the raw capability facts, which is the
        // verdict this page gave before reach existed: narrower, never wrong in
        // the unsafe direction.
        let mut wanted: Vec<String> = harnesses
            .iter()
            .flat_map(|h| h.def.requires.iter().map(|r| r.to_string()))
            .collect();
        wanted.extend(
            crate::fitness::score::fitness_slots()
                .into_iter()
                .flat_map(|s| s.requires),
        );
        wanted.sort();
        wanted.dedup();
        let reach = ((deps.reach)(model.clone(), wanted))
            .await
            .unwrap_or_default();

        let report = crate::fitness::score::score_fitness(
            &crate::fitness::score::FitnessInput {
                sweep: &effective,
                harnesses: &harnesses,
                capabilities: &capabilities_of(rows.iter().find(|r| r.id == model)),
                reach: Some(&reach),
                guard_baseline: Some(&crate::fitness::observed::guard_baseline(&observed)),
                floors: None,
            },
            &((deps.bind_slots)(harnesses.clone())).await?,
        );

        let at = (deps.now_iso)();
        let (kept, dropped) = drilldown(&effective.cases, DRILLDOWN_CAP);
        // The tiers this RECORD now speaks for, which is what an admin reads —
        // not the tiers this run happened to buy.
        let carried_tiers: Vec<TierId> = TIER_IDS
            .iter()
            .copied()
            .filter(|t| ran.contains(t) || prior.as_ref().is_some_and(|p| p.tiers.contains(t)))
            .collect();
        let mut tier_at: HashMap<TierId, String> = prior
            .as_ref()
            .map(|p| p.tier_at.clone())
            .unwrap_or_default();
        for t in &ran {
            tier_at.insert(*t, at.clone());
        }
        let record = FitnessRecord {
            model: model.clone(),
            at: at.clone(),
            tiers: carried_tiers,
            tier_at,
            report: report.clone(),
            harnesses: effective.harnesses.clone(),
            // A dozen provocations, of which only the ones the model FELL for
            // carry a transcript. Kept whole: this is the tier whose
            // drill-down an admin is most likely to need in order to justify a
            // decision.
            cases: kept,
            dropped_cases: dropped,
            probes: probes
                .clone()
                .or(prior.as_ref().and_then(|p| p.probes.clone())),
            adversarial: adversarial
                .clone()
                .or(prior.as_ref().and_then(|p| p.adversarial.clone())),
            sweep: FitnessSweepSummary {
                state: sweep_state_str(&effective.state),
                done: effective.done,
                total: effective.total,
                error: effective.error.clone(),
                unfixtured: effective.unfixtured.clone(),
                concurrency: effective.concurrency.clone(),
            },
        };
        ((deps.write_setting)(
            record_key(&model),
            serde_json::to_value(&record).map_err(|e| e.to_string())?,
        ))
        .await?;

        let prior_entry = match read_index(&deps).await {
            Ok(idx) => {
                let stored = stored_id_for(&model, &read_index(&deps).await.unwrap_or_default());
                idx.get(&stored).cloned()
            }
            Err(_) => None,
        };
        let entry = index_entry_of(IndexEntryParts {
            model: &model,
            at: &at,
            ran: &ran,
            requested: &tiers,
            sweep: &effective,
            report: &report,
            probes: probes
                .as_ref()
                .or(prior.as_ref().and_then(|p| p.probes.as_ref())),
            adversarial: adversarial
                .as_ref()
                .or(prior.as_ref().and_then(|p| p.adversarial.as_ref())),
            previous_speed: prior_entry.and_then(|e| e.speed),
        });
        let stored = deps
            .setting::<FitnessIndex>(INDEX_KEY, FitnessIndex::new())
            .await?;
        let mut next = stored;
        next.insert(model.clone(), entry);
        let (index, evicted) = evict_archive(&next, KEEP_MODELS);
        for stale in &evicted {
            let _ = ((deps.write_setting)(record_key(stale), Value::Null)).await;
        }
        ((deps.write_setting)(
            INDEX_KEY.to_string(),
            serde_json::to_value(&index).map_err(|e| e.to_string())?,
        ))
        .await?;

        write_status(
            FitnessRunStatus {
                state: FitnessRunState::Done,
                model: Some(model.clone()),
                tiers: tiers.clone(),
                phase: None,
                started_at: Some(started_at.clone()),
                finished_at: Some((deps.now_iso)()),
                error: None,
                heartbeat_at: None,
            },
            &deps,
        )
        .await;
        Ok(())
    }
    .await;

    if let Err(e) = outcome {
        write_status(
            FitnessRunStatus {
                state: FitnessRunState::Error,
                model: Some(model.clone()),
                tiers: tiers.clone(),
                phase: None,
                started_at: Some(started_at.clone()),
                finished_at: Some((deps.now_iso)()),
                error: Some(e),
                heartbeat_at: None,
            },
            &deps,
        )
        .await;
    }
    runs_map().lock().unwrap().remove(&model);
    // The request is spent once the run is over; leaving it would stop the next
    // Start before it began.
    clear_stop_request(&model, &deps).await;
}

/// `EvalSweepState` has no `as_str` (it is serde-only in evals.rs), and this
/// summary wants the exact wire spelling the archive has always carried.
fn sweep_state_str(state: &crate::fitness::evals::EvalSweepState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "idle".to_string())
}

// ── Payloads ─────────────────────────────────────────────────────────────────

/// Merge one run's persisted status with its tier-2 case counter. The counter
/// belongs to tier 2 and is READ from it, never mirrored — two progress
/// counters for one run is how they come to disagree.
/// The runs blob AS STORED, key order intact — `fitness_runs` explains why the
/// wire rows are built from this and not from the typed read.
async fn read_runs_raw(deps: &SurfaceDeps) -> Result<serde_json::Map<String, Value>, String> {
    let raw = ((deps.read_setting)(RUNS_KEY.to_string(), serde_json::json!({}))).await?;
    let mut out = raw.as_object().cloned().unwrap_or_default();
    // The legacy single-run row folds in the same way the typed read folds it.
    let legacy = ((deps.read_setting)(STATUS_KEY.to_string(), Value::Null)).await?;
    if let Some(model) = legacy.get("model").and_then(|m| m.as_str())
        && !out.contains_key(model)
    {
        out.insert(model.to_string(), legacy);
    }
    Ok(out)
}

/// The idle status row AS THE WIRE WRITES IT — the idle literal's field order
/// (`state, model, tiers, phase`) with the view fields appended. This row is
/// never stored, so unlike a stored run row it does NOT ride jsonb key order.
fn idle_view() -> Value {
    serde_json::json!({
        "state": "idle",
        "model": null,
        "tiers": [],
        "phase": null,
        "done": 0,
        "total": 0,
        "harness": null,
        "sweepState": "idle",
    })
}

/// ONE candidate's status, or the idle row. Kept because three callers ask
/// about a specific model and would otherwise each filter the list.
pub async fn fitness_status(model: Option<&str>, deps: &SurfaceDeps) -> Result<Value, String> {
    let all = fitness_runs(deps).await?;
    Ok(all
        .runs
        .into_iter()
        .find(|r| r.get("model").and_then(Value::as_str) == model)
        .unwrap_or_else(idle_view))
}

pub async fn fitness_runs(deps: &SurfaceDeps) -> Result<FitnessRunsView, String> {
    // THE WIRE ROWS ARE BUILT FROM THE RAW BLOB, not the typed read. A stored
    // row's keys ride jsonb's canonical order (see `upgrade_record_value`),
    // and `{ ...status, done, total, harness, sweepState }` keeps every
    // stored key where it was and appends the four view fields — so a typed
    // struct, which serializes in declaration order, would reorder the row
    // even with every value correct. The typed parse is still needed for the
    // decisions (staleness, sweep gating, in-flight counting).
    let statuses = read_runs_raw(deps).await?;
    let typed: HashMap<String, FitnessRunStatus> = statuses
        .iter()
        .filter_map(|(k, v)| {
            serde_json::from_value(v.clone())
                .ok()
                .map(|t| (k.clone(), t))
        })
        .collect();
    // The checkpoint rows are asked for BY NAME, from the models the status
    // store already knows about — one small row each, rather than one shared
    // blob that grew with every case of every concurrent run.
    let sweeps = ((deps.eval_sweep_statuses)(typed.keys().cloned().collect()))
        .await
        .unwrap_or_default();
    // A STUCK RUN REPORTS AS FAILED, HERE, on the read every surface goes
    // through.
    //
    // A run is a task inside a process and its status is a row in the
    // database, so a restart used to leave the row claiming `running` for
    // ever: the panel counted it against the concurrency limit, `full` went
    // true, and Stop wrote a request that no living thing would ever read.
    //
    // Reported rather than REWRITTEN, because a read is not the place to take
    // a durable decision — two instances reading at once would both write, and
    // a run that is merely slow would have its row destroyed by whoever looked
    // first. `stop_fitness_run` does the writing, and it does it because
    // somebody asked.
    let now = crate::agent_auth::iso_to_epoch_ms(&(deps.now_iso)()).unwrap_or(0);
    let mut runs: Vec<Value> = statuses
        .into_iter()
        .map(|(key, raw)| {
            // A stored row is an object; anything else spreads to nothing
            // (`{ ...status }`), so it becomes the four view fields alone.
            let mut row = raw.as_object().cloned().unwrap_or_default();
            let t = typed.get(&key);
            if t.map(|s| stale_run(s, now)).unwrap_or(false) {
                row.insert("state".into(), serde_json::json!("error"));
                row.insert(
                    "error".into(),
                    serde_json::json!("interrupted: the server restarted or the run died"),
                );
            }
            let running = row.get("state").and_then(Value::as_str) == Some("running");
            let sweep = t
                .and_then(|s| s.model.as_deref())
                .and_then(|m| sweeps.get(m));
            // ONLY WHEN THIS RUN IS ACTUALLY SWEEPING. The checkpoint is per
            // model and outlives the run that wrote it, so a probes-only run
            // on a model swept earlier displayed that older sweep's counter —
            // "probes 247/247" on a run with no fixtures in it at all.
            let live = running
                && sweep.is_some()
                && t.map(|s| s.tiers.contains(&TierId::Evals)).unwrap_or(false);
            row.insert(
                "done".into(),
                serde_json::json!(if live {
                    sweep.map(|s| s.done).unwrap_or(0)
                } else {
                    0
                }),
            );
            row.insert(
                "total".into(),
                serde_json::json!(if live {
                    sweep.map(|s| s.total).unwrap_or(0)
                } else {
                    0
                }),
            );
            row.insert(
                "harness".into(),
                serde_json::json!(
                    live.then(|| sweep.and_then(|s| s.harness.clone()))
                        .flatten()
                ),
            );
            row.insert(
                "sweepState".into(),
                serde_json::json!(
                    sweep
                        .map(|s| sweep_state_str(&s.state))
                        .unwrap_or_else(|| "idle".to_string())
                ),
            );
            Value::Object(row)
        })
        .collect();
    // Running first, then most recently started — an admin watching three
    // sweeps wants the live ones at the top, not whichever id sorts first.
    // ISO strings of one format order identically as bytes and under
    // `localeCompare` — either comparison gives this order.
    runs.sort_by(|a, b| {
        let live = (b.get("state").and_then(Value::as_str) == Some("running")) as i8
            - (a.get("state").and_then(Value::as_str) == Some("running")) as i8;
        if live != 0 {
            return live.cmp(&0);
        }
        b.get("startedAt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(a.get("startedAt").and_then(Value::as_str).unwrap_or(""))
    });
    let in_flight = runs
        .iter()
        .filter(|r| r.get("state").and_then(Value::as_str) == Some("running"))
        .count();
    Ok(FitnessRunsView {
        runs,
        max: MAX_CONCURRENT_RUNS,
        full: in_flight >= MAX_CONCURRENT_RUNS,
    })
}

/// The archive as every READER wants it — re-keyed onto the ids the catalog
/// offers. The writers (`run_fitness`, `evict_archive`, `forget_model`) keep
/// reading it raw: they operate on stored state, and re-keying under them would
/// archive a run twice or delete the wrong row.
pub async fn read_index(deps: &SurfaceDeps) -> Result<FitnessIndex, String> {
    let index = deps
        .setting::<FitnessIndex>(INDEX_KEY, FitnessIndex::new())
        .await?;
    let catalog = ((deps.models)()).await.unwrap_or_default();
    Ok(if catalog.is_empty() {
        index
    } else {
        canonical_index(&index, &catalog)
    })
}

/// The index AS THE MATRIX SENDS IT — raw values. The matrix view serializes
/// the stored entries verbatim, so the entries' own keys must ride in the
/// stored jsonb order; a typed entry would re-order them for the same reason
/// `upgrade_record_value` exists.
pub async fn read_index_raw(deps: &SurfaceDeps) -> Result<serde_json::Map<String, Value>, String> {
    let raw = ((deps.read_setting)(INDEX_KEY.to_string(), serde_json::json!({}))).await?;
    let stored = raw.as_object().cloned().unwrap_or_default();
    let catalog = ((deps.models)()).await.unwrap_or_default();
    if catalog.is_empty() {
        return Ok(stored);
    }
    // `canonical_index` on raw values — same two passes, same comment.
    let mut out = serde_json::Map::new();
    for (model, entry) in &stored {
        let id = crate::model::access::canonical_model_id(model, &catalog);
        if id == *model || !out.contains_key(&id) {
            out.insert(id, entry.clone());
        }
    }
    for (model, entry) in &stored {
        if catalog.iter().any(|m| &m.id == model) {
            out.insert(model.clone(), entry.clone());
        }
    }
    Ok(out)
}

/// The value view's edges, bound to this module's injected ones so a test can
/// drive the whole page from `SurfaceDeps` and never learn a second deps
/// shape. `value.rs` itself stays free of every real import — it is arithmetic
/// over what it is handed.
pub async fn read_value(deps: &SurfaceDeps) -> Result<crate::fitness::value::ValueView, String> {
    let d = deps.clone();
    let d2 = deps.clone();
    let d3 = deps.clone();
    let d4 = deps.clone();
    let d5 = deps.clone();
    let d6 = deps.clone();
    crate::fitness::value::value_view(&crate::fitness::value::ValueDeps {
        observed: Arc::new(move || {
            let d = d.clone();
            Box::pin(async move { (d.observed_harnesses)(None).await })
        }),
        harnesses: Arc::new(move || {
            let d = d2.clone();
            Box::pin(async move { (d.harnesses)().await })
        }),
        bindings: Arc::new(move |registry| {
            let d = d3.clone();
            Box::pin(async move { (d.bind_slots)(registry).await })
        }),
        index: Arc::new(move || {
            let d = d4.clone();
            Box::pin(async move { read_index(&d).await })
        }),
        budget: Arc::new(move || {
            let d = d5.clone();
            Box::pin(async move { d.setting(BUDGET_KEY, TokenBudget::new()).await })
        }),
        price: Arc::new(move |model| {
            let d = d6.clone();
            Box::pin(async move { price_of(&model, &d).await })
        }),
        record: Arc::new({
            let d7 = deps.clone();
            move |model| {
                let deps = d7.clone();
                Box::pin(async move {
                    let raw = deps
                        .setting::<Option<FitnessRecord>>(&record_key(&model), None)
                        .await?;
                    Ok(
                        upgrade_record(raw).map(|r| crate::fitness::value::ArchivedRecord {
                            report: r.report,
                            harnesses: r.harnesses,
                        }),
                    )
                })
            }
        }),
        window_days: crate::fitness::observed::DEFAULT_WINDOW_DAYS,
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixView {
    pub slots: Vec<SlotView>,
    pub models: Vec<ModelRow>,
    /// Raw values, not typed entries: the stored entries go on the wire
    /// verbatim, so their keys ride the stored jsonb order — same reason as
    /// `upgrade_record_value`.
    pub index: serde_json::Map<String, Value>,
    /// EVERY RUN, not one. Up to `max` candidates are tested at once, and a
    /// single `status` field could only ever draw one of them — which is how
    /// an admin who started three would watch two of them vanish.
    pub runs: Vec<Value>,
    pub max: usize,
    pub full: bool,
    pub thresholds: Thresholds,
    pub registry: MatrixRegistry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixRegistry {
    pub harnesses: usize,
    pub fixtures: i64,
    /// TIER 3 IS PART OF THE BATTERY AND IS COUNTED HERE. A page reading
    /// "26 harnesses · 247 fixtures" over a suite that also runs 19 safety
    /// provocations understates what a full run measures, and the adversarial
    /// corpus is the half an admin is least likely to know about.
    pub provocations: usize,
    pub unfixtured: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesView {
    pub models: Vec<ModelRow>,
    /// RAW entries, for the same reason the matrix's index is raw: the stored
    /// keys ride jsonb's canonical order and a typed entry would re-serialize
    /// them in declaration order (see `fitness_runs`).
    pub index: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateView {
    pub estimate: RunEstimate,
    /// `ADVERSARY_REQUIREMENT` verbatim — the `{capabilities, note}` object
    /// the estimate sends next to the adversary picker.
    pub adversary_requirement: Value,
}

/// A RUN IN FLIGHT, as the drill-down can show it.
///
/// The sweep checkpoints every case as it lands, so the audit trail an admin
/// wants while a run is working already exists — it was simply never sent. The
/// drill-down read only the ARCHIVED record, so opening a model mid-run said
/// "no run on record" about a sweep that was at that moment 140 fixtures in.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRun {
    pub state: String,
    pub phase: Option<String>,
    pub done: i64,
    pub total: i64,
    /// The harness being swept right now, for the line above the list.
    pub harness: Option<String>,
    pub cases: Vec<crate::fitness::evals::EvalCaseScore>,
    pub dropped: i64,
    /// THE FEED: one line per landed case, every case, cheap. `cases` above is
    /// the drill-down sample; this is the thing that shows a sweep moving.
    pub log: Vec<EvalLogLine>,
    /// THE CASES RUNNING RIGHT NOW, with their turns as they happen. Empty
    /// when the sweep belongs to another instance — see `in_flight_for`. That
    /// reads as an empty panel, never as a wrong one. Several at once: a sweep
    /// runs `concurrency` cases in parallel.
    pub current: Vec<crate::fitness::evals::InFlightCase>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailView {
    pub model: String,
    /// The archived record as the RAW stored value, upgraded — see
    /// [`upgrade_record_value`]: the drill-down must ride the wire in the
    /// archive's own (jsonb) key order, which a typed struct cannot promise.
    pub record: Option<Value>,
    /// Non-null only while this candidate is being tested.
    pub live: Option<LiveRun>,
    /// THE CONSOLE, WHICH OUTLIVES THE RUN. Live it is the same lines
    /// `live.log` carries; afterwards it is rebuilt from the archived record
    /// and whatever the in-memory tier feed still holds, so the terminal
    /// somebody was watching is still there to read back when the sweep
    /// finishes.
    ///
    /// Separate from `live` on purpose: `live` is what the panel POLLS on, and
    /// hanging a finished run's console off it would poll forever.
    pub console_log: Vec<EvalLogLine>,
    pub observed: Vec<crate::fitness::observed::ObservedHarness>,
    pub observed_model: Option<crate::fitness::observed::ObservedModel>,
    pub divergences: Vec<crate::fitness::observed::Divergence>,
    pub thresholds: Thresholds,
}

/// What the GET verb was asked for, already parsed off the query string by the
/// route. Everything below this line is a decision; the parsing above it is
/// HTTP plumbing and stays in the route.
#[derive(Debug, Clone, Default)]
pub struct FitnessQuery {
    pub view: String,
    pub model: Option<String>,
    pub tiers: Option<String>,
    pub adversary: Option<String>,
    pub only: Option<String>,
    /// `?reprobe=1` — price the run that RE-MEASURES capabilities we already
    /// have, so the estimate matches the box the admin just ticked.
    pub reprobe: bool,
    /// `?run=<iso>` — which archived run's transcripts to read. Omitted means
    /// the newest one on record.
    pub run: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptView {
    pub model: String,
    pub runs: Vec<crate::fitness::transcripts::TranscriptRun>,
    pub cases: Vec<crate::fitness::transcripts::Transcript>,
}

/// The GET verb's answer. `Err` is a 400 with a sentence — the route maps it;
/// nothing here knows about codes.
pub async fn read_fitness(
    query: &FitnessQuery,
    pg: &sqlx::PgPool,
    deps: &SurfaceDeps,
) -> Result<Value, String> {
    let model = query.model.as_deref();

    if query.view == "capabilities" {
        // The index rides along: the panels that PICK a model need the band
        // its last run gave the slot they are assigning, and a second round
        // trip for two dozen small cell maps would be a request per panel per
        // open.
        let (models, index) = tokio::try_join!(model_rows(deps), read_index_raw(deps))?;
        let body = CapabilitiesView { models, index };
        return serde_json::to_value(&body).map_err(|e| e.to_string());
    }

    if query.view == "transcripts" {
        // THE AUDIT VIEW, and it is its own request on purpose. A full run is
        // every case with its prompt, its reply, its turns and its tool calls
        // — hundreds of kilobytes — and the fitness page is polled every three
        // seconds while a sweep is in flight. Nothing that renders by default
        // may touch this. The transcripts table is read directly rather than
        // through an edge: it is the one store this module owns outright.
        let Some(model) = model else {
            return Err("model is required".to_string());
        };
        let body = TranscriptView {
            model: model.to_string(),
            runs: crate::fitness::transcripts::transcript_runs(pg, model)
                .await
                .map_err(|e| e.to_string())?,
            cases: crate::fitness::transcripts::read_transcripts(pg, model, query.run.as_deref())
                .await
                .map_err(|e| e.to_string())?,
        };
        return serde_json::to_value(&body).map_err(|e| e.to_string());
    }

    if query.view == "health" {
        // ACROSS EVERY ARCHIVED MODEL, which is the only way to tell a broken
        // fixture from a hard one. Its own request: it reads one record per
        // tested candidate, and the matrix is polled every three seconds while
        // a run is in flight.
        let index = read_index(deps).await?;
        let mut records: Vec<FitnessRecord> = Vec::new();
        for id in index.keys() {
            if let Some(rec) = upgrade_record(
                deps.setting::<Option<FitnessRecord>>(&record_key(id), None)
                    .await
                    .unwrap_or(None),
            ) {
                records.push(rec);
            }
        }
        let runs: Vec<crate::fitness::health::HealthInput<'_>> = records
            .iter()
            .map(|rec| crate::fitness::health::HealthInput {
                model: &rec.model,
                cases: &rec.cases,
            })
            .collect();
        let body = crate::fitness::health::summarize(&runs);
        return serde_json::to_value(&body).map_err(|e| e.to_string());
    }

    if query.view == "value" {
        // Its own view, not a field on the matrix: it costs a telemetry query
        // and a price lookup per tested model, and the matrix is polled every
        // 3s while a run is in flight. An admin opens this one on purpose.
        let body = read_value(deps).await?;
        return serde_json::to_value(&body).map_err(|e| e.to_string());
    }

    if query.view == "estimate" {
        let Some(model) = model else {
            return Err("model is required".to_string());
        };
        let tiers: Vec<TierId> = query
            .tiers
            .as_deref()
            .unwrap_or("probes,evals")
            .split(',')
            .filter(|t| is_tier_id(t))
            .map(|t| match t {
                "probes" => TierId::Probes,
                "adversarial" => TierId::Adversarial,
                _ => TierId::Evals,
            })
            .collect();
        if tiers.is_empty() {
            return Err("pick at least one tier".to_string());
        }
        let only: Option<Vec<String>> = query.only.as_deref().map(|o| {
            o.split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        });
        let body = EstimateView {
            estimate: estimate_run(
                &EstimateRequest {
                    model: model.to_string(),
                    tiers,
                    adversary_model: query.adversary.clone(),
                    only: only.filter(|o| !o.is_empty()),
                    reprobe: query.reprobe,
                },
                deps,
            )
            .await?,
            adversary_requirement: serde_json::json!({
                "capabilities": crate::fitness::adversarial::ADVERSARY_REQUIREMENT.0,
                "note": crate::fitness::adversarial::ADVERSARY_REQUIREMENT.1,
            }),
        };
        return serde_json::to_value(&body).map_err(|e| e.to_string());
    }

    if query.view == "detail" {
        return read_fitness_detail(query, deps).await;
    }

    let (models, index, runs_view, shape) = tokio::join!(
        model_rows(deps),
        // RAW for the wire field — a stored index entry's keys ride jsonb's
        // canonical order and a typed struct would re-serialize them in
        // declaration order (see `fitness_runs`).
        read_index_raw(deps),
        fitness_runs(deps),
        tier2_shape(None, deps)
    );
    let shape = shape?;
    let runs_view = runs_view?;
    let body = MatrixView {
        slots: slot_views(),
        models: models?,
        index: index?,
        // The runs the page draws, plus the cap it disables Start at. Several
        // at once is the feature; one `status` field could only ever show one.
        runs: runs_view.runs,
        max: runs_view.max,
        full: runs_view.full,
        thresholds: thresholds(),
        registry: MatrixRegistry {
            harnesses: shape.harnesses.len(),
            fixtures: shape.fixtures,
            provocations: crate::fitness::adversarial::SEEDS.len(),
            unfixtured: shape
                .harnesses
                .iter()
                .filter(|h| h.eval_names().next().is_none())
                .map(|h| h.def.id.to_string())
                .collect(),
        },
    };
    serde_json::to_value(&body).map_err(|e| e.to_string())
}

/// The detail view, split out of `read_fitness` only because it is the one
/// branch with a live half to assemble.
async fn read_fitness_detail(query: &FitnessQuery, deps: &SurfaceDeps) -> Result<Value, String> {
    let Some(model) = query.model.as_deref() else {
        return Err("model is required".to_string());
    };
    // Through `stored_id_for`, because the row an admin clicked is keyed by the
    // id the CATALOG offers and the report is filed under the id the RUN used.
    // Reading it directly showed "no run on record" for a model whose verdicts
    // were on screen a click earlier.
    //
    // The read is RAW, not typed: this record is the one place the archive
    // itself goes on the wire, and the wire must carry the stored jsonb key
    // order. The typed view consumers (console log, divergences) parse the
    // upgraded value — the same object the wire carries.
    let record_wire = {
        let raw = ((deps.read_setting)(
            record_key(&stored_id_for(model, &read_index(deps).await?)),
            Value::Null,
        ))
        .await
        .unwrap_or(Value::Null);
        if raw.is_null() {
            None
        } else {
            Some(upgrade_record_value(raw))
        }
    };
    let record: Option<FitnessRecord> = record_wire
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    // Production telemetry is ADVISORY and is fetched even with no bench
    // record: "this model is running in production and has never been tested"
    // is one of the more useful things this page can say.
    let observed = ((deps.observed_harnesses)(Some(model.to_string())))
        .await
        .unwrap_or_default();
    let models = ((deps.observed_models)()).await.unwrap_or_default();
    // THE LIVE HALF. Only while this candidate is actually running: a finished
    // run has an archived record, which is the better thing to read, and a
    // checkpoint outlives the run that wrote it. The TYPED read, not the wire
    // builder — the console needs the counters as numbers, and the wire's raw
    // rows exist for serialization order (see `fitness_runs`).
    let statuses = read_runs(deps).await.unwrap_or_default();
    let now = crate::agent_auth::iso_to_epoch_ms(&(deps.now_iso)()).unwrap_or(0);
    let running = statuses
        .values()
        .find(|s| {
            s.model.as_deref() == Some(model)
                && s.state == FitnessRunState::Running
                && !stale_run(s, now)
        })
        .cloned();
    let mut live: Option<LiveRun> = None;
    if let Some(running) = running {
        let sweeps = ((deps.eval_sweep_statuses)(vec![model.to_string()]))
            .await
            .unwrap_or_default();
        let sweep = sweeps.get(model);
        // ONLY WHEN THIS RUN IS ACTUALLY SWEEPING — the same rule
        // `fitness_runs` applies to the counter, and the console had the same
        // bug. The sweep checkpoint is per model and OUTLIVES the run that
        // wrote it, so a probes-only run on a model swept earlier opened its
        // console with two hundred and forty-seven fixture lines from a run
        // that finished hours ago, above the probes it was actually running.
        let sweeping = running.tiers.contains(&TierId::Evals);
        let cases: Vec<crate::fitness::evals::EvalCaseScore> = if sweeping {
            sweep.map(|s| s.cases.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let (kept, dropped) = live_cases(&cases);
        live = Some(LiveRun {
            state: running.state.as_str().to_string(),
            phase: running.phase.clone(),
            done: if sweeping {
                sweep.map(|s| s.done).unwrap_or(0)
            } else {
                0
            },
            total: if sweeping {
                sweep.map(|s| s.total).unwrap_or(0)
            } else {
                0
            },
            harness: if sweeping {
                sweep.and_then(|s| s.harness.clone())
            } else {
                None
            },
            cases: kept,
            dropped,
            // ONE FEED, IN THE ORDER THINGS HAPPENED. The three tiers run in
            // sequence and the console is read as a timeline, so concatenating
            // "sweep cases, then the rest" printed tier 1's probes UNDERNEATH
            // the fixtures that ran after them.
            log: run_log(&cases, &crate::fitness::live_feed::live_feed_for(model)),
            current: crate::fitness::evals::in_flight_for(model),
        });
    }

    let console_log = live.as_ref().map(|l| l.log.clone()).unwrap_or_else(|| {
        run_log(
            record.as_ref().map(|r| r.cases.as_slice()).unwrap_or(&[]),
            &archived_tier_log(
                record.as_ref(),
                &crate::fitness::live_feed::live_feed_for(model),
            ),
        )
    });
    let divergences = match &record {
        Some(record) => crate::fitness::observed::divergences(
            model,
            &record.harnesses,
            &observed,
            &crate::fitness::observed::DivergenceOptions::default(),
        ),
        None => Vec::new(),
    };
    let body = DetailView {
        model: model.to_string(),
        record: record_wire,
        live,
        // Live: exactly what the terminal is already showing. Finished: the
        // same shape rebuilt FROM THE ARCHIVE, so the console survives the run
        // — and a restart. The in-memory feed is preferred while it lasts
        // because it carries per-unit timings the archived reports do not.
        console_log,
        observed_model: models.into_iter().find(|m| m.model == model),
        divergences,
        observed,
        thresholds: thresholds(),
    };
    serde_json::to_value(&body).map_err(|e| e.to_string())
}

// ── The write verbs ──────────────────────────────────────────────────────────

/// The Start verb's answer. `Busy` is a 409 and `Rejected(String)` a 400; the
/// route decides that, this only says which.
#[derive(Debug)]
pub enum StartOutcome {
    /// Either this candidate is already running — the second press of Start
    /// means "show me the run", not "start a second one", the same call the
    /// tier-2 sweep makes for itself — or every slot is taken. `refusal` tells
    /// the two apart so the route can say which.
    Busy {
        refusal: RunRefusal,
        status: Value,
        runs: FitnessRunsView,
    },
    Rejected(String),
    Started {
        status: Value,
        runs: FitnessRunsView,
    },
}

/// The Start verb's request, already parsed by the route.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub model: String,
    pub tiers: Vec<TierId>,
    pub adversary_model: Option<String>,
    pub only: Option<Vec<String>>,
    pub restart: bool,
    pub reprobe: bool,
    pub concurrency: Option<usize>,
    pub retry_failed: bool,
    pub supplement: bool,
}

/// Validate, claim the run slot, and detach the run.
pub async fn start_fitness_run(req: StartRequest, deps: Arc<SurfaceDeps>) -> StartOutcome {
    // A fast 409 for the common case, before the catalog reads. Not the door —
    // `claim_run` below is, because only it is synchronous with the claim.
    let early = {
        let runs = runs_map().lock().unwrap();
        if runs.contains_key(&req.model) {
            Some(RunRefusal::AlreadyRunning)
        } else if runs.len() >= MAX_CONCURRENT_RUNS {
            Some(RunRefusal::AtCapacity)
        } else {
            None
        }
    };
    if let Some(refusal) = early {
        let (status, runs) = join_status_runs(Some(&req.model), &deps).await;
        return StartOutcome::Busy {
            refusal,
            status,
            runs,
        };
    }

    let rows = match model_rows(&deps).await {
        Ok(rows) => rows,
        Err(e) => return StartOutcome::Rejected(e),
    };
    if !rows.iter().any(|r| r.id == req.model) {
        return StartOutcome::Rejected("that model is not on the gateway".to_string());
    }
    if let Some(adversary) = &req.adversary_model {
        if !rows.iter().any(|r| &r.id == adversary) {
            return StartOutcome::Rejected(
                "that adversary model is not on the gateway".to_string(),
            );
        }
        if adversary == &req.model {
            // A model grading its own resistance is the who-judges-the-judge
            // regress with the stakes turned up (adversarial.rs says so in
            // prose; this is the door).
            return StartOutcome::Rejected(
                "the adversary must be a different model than the candidate".to_string(),
            );
        }
    }

    let opts = StartOptions {
        model: req.model.clone(),
        tiers: req.tiers.clone(),
        adversary_model: req.adversary_model.clone(),
        restart: req.restart,
        reprobe: req.reprobe,
        concurrency: req.concurrency,
        retry_failed: req.retry_failed,
        supplement: req.supplement,
        only: req.only.clone().filter(|o| !o.is_empty()),
    };
    // Claimed here, with no await between the check and the claim.
    if let Some(refusal) = claim_run(&opts.model) {
        let (status, runs) = join_status_runs(Some(&opts.model), &deps).await;
        return StartOutcome::Busy {
            refusal,
            status,
            runs,
        };
    }
    // A STOP REQUEST BELONGS TO THE RUN IT STOPPED. Clearing it only when a run
    // ENDS leaves it set for any run that never got to finish — a stopped
    // sweep, a process killed mid-run — and the next Start on that model then
    // stops itself after one case, having read a flag meant for a run that is
    // already gone.
    clear_stop_request(&opts.model, &deps).await;
    let _ = write_run_status(
        &FitnessRunStatus {
            state: FitnessRunState::Running,
            model: Some(opts.model.clone()),
            tiers: opts.tiers.clone(),
            phase: opts.tiers.first().map(|t| t.as_str().to_string()),
            started_at: Some((deps.now_iso)()),
            finished_at: None,
            error: None,
            heartbeat_at: None,
        },
        &deps,
    )
    .await;
    let spawned = deps.clone();
    tokio::spawn(async move {
        run_fitness(opts, spawned).await;
    });
    let (status, runs) = join_status_runs(Some(&req.model), &deps).await;
    StartOutcome::Started { status, runs }
}

async fn join_status_runs(model: Option<&str>, deps: &SurfaceDeps) -> (Value, FitnessRunsView) {
    let (status, runs) = tokio::join!(fitness_status(model, deps), fitness_runs(deps));
    (
        status.unwrap_or_else(|_| idle_view()),
        runs.unwrap_or(FitnessRunsView {
            runs: Vec::new(),
            max: MAX_CONCURRENT_RUNS,
            full: false,
        }),
    )
}

pub struct StopResult {
    /// Did the REQUEST land? Not "did this instance own the run" — a button
    /// that reported false while the sweep obediently stopped was half of the
    /// bug the persisted stop request was written to fix.
    pub stopped: bool,
    pub status: Value,
    pub runs: FitnessRunsView,
}

/// Stop one candidate's run, or — with no model — every run in flight.
///
/// TWO STOPS PER RUN, because there are two things running: the tier-2 sweep,
/// which honors it at a case boundary and stays RESUMABLE, and the tier loop,
/// which honors it by not buying the tiers that have not started.
pub async fn stop_fitness_run(model: Option<&str>, deps: &SurfaceDeps) -> StopResult {
    // THE TARGETS COME FROM THE RAW PERSISTED ROWS, not from the in-process
    // map and not from `fitness_runs`.
    //
    // Not the map, because it is empty in any instance that did not start the
    // run — the case where Stop used to do nothing at all.
    //
    // AND NOT `fitness_runs`, which is the subtler one and was a bug for
    // exactly one commit: that view reports a stale row as `error` so the
    // panel stops counting a dead run, which means Stop-all could no longer
    // SEE an orphan and reported `stopped: false` while the row sat there
    // saying `running` for ever. The sanitised view is for reading; the thing
    // being stopped is the row.
    let persisted_now = read_runs(deps).await.unwrap_or_default();
    let live: Vec<String> = persisted_now
        .values()
        .filter(|r| r.state == FitnessRunState::Running)
        .filter_map(|r| r.model.clone())
        .collect();
    let targets: Vec<String> = match model {
        None => live,
        Some(m) => vec![m.to_string()],
    };

    // Written FIRST, so the request outlives this process whatever happens
    // next.
    if !targets.is_empty() {
        let asked = deps
            .setting::<Vec<String>>(STOP_KEY, Vec::new())
            .await
            .unwrap_or_default();
        let mut next: Vec<String> = asked;
        for t in &targets {
            if !next.iter().any(|m| m == t) {
                next.push(t.clone());
            }
        }
        if let Ok(v) = serde_json::to_value(&next) {
            let _ = ((deps.write_setting)(STOP_KEY.to_string(), v)).await;
        }
        // The instance that TOOK the request must not then serve a cached "no"
        // to its own sweep for the next two seconds.
        *stop_cache().lock().unwrap() = (0, std::collections::HashSet::new());
    }

    // Then the fast path, for a run this instance is holding.
    for m in &targets {
        if let Some(slot) = runs_map().lock().unwrap().get_mut(m) {
            *slot = true;
        }
        (deps.stop_eval_sweep)(Some(m.clone()));
    }

    // AND THE ORPHANS, which are the whole reason Stop could look broken. A
    // stop request is a note left for a running loop to read between cases; a
    // run whose process died reads nothing, so the request sat there and the
    // row kept saying `running`. Pressing Stop on one of those has exactly one
    // sensible meaning — end it — so it is ended here rather than asked.
    //
    // ONLY THE STALE ONES. A live run belonging to ANOTHER instance is not
    // orphaned, and killing its row from here would report a sweep as failed
    // while it was still spending money. The heartbeat is what tells them
    // apart.
    let now_ms = crate::agent_auth::iso_to_epoch_ms(&(deps.now_iso)()).unwrap_or(0);
    for m in &targets {
        let Some(row) = persisted_now.get(m) else {
            continue;
        };
        if !stale_run(row, now_ms) {
            continue;
        }
        let mut dead = row.clone();
        dead.state = FitnessRunState::Error;
        dead.phase = None;
        dead.error = Some("interrupted: the server restarted or the run died".to_string());
        dead.finished_at = Some((deps.now_iso)());
        write_run_status(&dead, deps).await;
        // The note is pointless now, and left behind it would stop the NEXT run
        // on this model after one case — a bug this file has already had once.
        clear_stop_request(m, deps).await;
    }

    let (status, runs) = join_status_runs(model, deps).await;
    StopResult {
        stopped: !targets.is_empty(),
        status,
        runs,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearResult {
    /// Models whose results were removed.
    pub models: Vec<String>,
    /// Archived reports deleted.
    pub reports: i64,
    /// Transcript rows deleted.
    pub transcripts: i64,
}

/// WIPE RECORDED RESULTS so a candidate can be tested from nothing.
///
/// THIS IS NOT `forget_model`, and the difference is the whole reason both
/// exist. Forget throws away what we know a model CAN DO — probe facts,
/// measured once and true until the id is re-pointed at different weights —
/// and is the release valve for exactly that. This throws away what a RUN
/// FOUND: the report, its index entry, the resume ledger and the archived
/// transcripts. An admin who has just fixed a fixture wants the second one and
/// emphatically not the first, because re-probing is nine calls they have
/// already paid for.
///
/// ALL FOUR, OR IT DOES NOT WORK. Clearing the report but leaving the sweep
/// status behind is the trap: the model reads as untested and the next Start
/// resumes into a run that is already complete, returning instantly having
/// bought nothing. Clearing the report but leaving the index entry behind
/// leaves the matrix pointing at a report that is gone, which is the one state
/// the detail route cannot serve.
///
/// `model: None` clears every tested candidate.
pub async fn clear_fitness_results(
    model: Option<&str>,
    deps: &SurfaceDeps,
) -> Result<ClearResult, String> {
    let index = deps
        .setting::<FitnessIndex>(INDEX_KEY, FitnessIndex::new())
        .await?;
    let targets: Vec<String> = match model {
        None => index.keys().cloned().collect(),
        Some(model) => {
            let catalog = ((deps.models)()).await.unwrap_or_default();
            let canonical = canonical_index(&index, &catalog);
            vec![stored_id_for(model, &canonical)]
        }
    };

    let mut reports = 0i64;
    for id in &targets {
        if index.contains_key(id) {
            reports += 1;
        }
        ((deps.write_setting)(record_key(id), Value::Null)).await?;
        // The resume ledger, which is the half everyone forgets.
        let _ = ((deps.clear_eval_status)(id.clone())).await;
    }
    let rest: FitnessIndex = index
        .into_iter()
        .filter(|(id, _)| !targets.contains(id))
        .collect();
    ((deps.write_setting)(
        INDEX_KEY.to_string(),
        serde_json::to_value(&rest).map_err(|e| e.to_string())?,
    ))
    .await?;
    let transcripts = match model {
        None => ((deps.clear_transcripts)(None)).await.unwrap_or(0),
        Some(_) => ((deps.clear_transcripts)(targets.first().cloned()))
            .await
            .unwrap_or(0),
    };
    Ok(ClearResult {
        models: targets,
        reports,
        transcripts,
    })
}

/// The Forget verb's answer. `Err` is a 400 with the sentence the admin reads;
/// a missing report is NOT an error — the valve is idempotent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetOk {
    /// The `endpoint:model` keys whose facts were dropped.
    pub keys: Vec<String>,
    pub models: Vec<ModelRow>,
    pub report: bool,
}

/// Audit 1.2's release valve, per endpoint:model rather than per id: a model id
/// re-pointed at different weights has facts about something else, and the
/// gateway's learned-parameter ratchet has no other way out.
///
/// IT FORGETS THE REPORT TOO, and until it did, the button did not appear to
/// work at all. Talaria records what it knows about a model in TWO places, and
/// this only ever cleared one of them: the capability facts and the archived
/// report. The confirm dialog has always promised "probe results ... are
/// deleted", which is the correct promise for a valve whose whole purpose is a
/// model id pointed at new weights — so the fix is to keep the promise rather
/// than narrow it. `write_setting(key, null)` is how `evict_archive` already
/// deletes a report; this uses the same door.
pub async fn forget_model(model: &str, deps: &SurfaceDeps) -> Result<ForgetOk, String> {
    let rows = model_rows(deps).await?;
    let Some(row) = rows.iter().find(|r| r.id == model) else {
        return Err("that model is not on the gateway".to_string());
    };
    let keys = keys_for(&row.id, row.qualified, &row.endpoints);
    for key in &keys {
        ((deps.forget)(key.clone())).await?;
    }

    // The index entry goes with the record, in that order: an index naming a
    // report that is already gone is the one state the detail route cannot
    // serve. RAW index, and the STORED id: Forget deletes rows, so it has to
    // name them the way they are actually filed. The button is pressed from a
    // canonical row, so the two can differ — and a Forget that deleted neither
    // while reporting success is exactly the bug this function was written to
    // fix.
    let index = deps
        .setting::<FitnessIndex>(INDEX_KEY, FitnessIndex::new())
        .await?;
    let catalog = ((deps.models)()).await.unwrap_or_default();
    let stored = stored_id_for(model, &canonical_index(&index, &catalog));
    let report = index.contains_key(&stored);
    ((deps.write_setting)(record_key(&stored), Value::Null)).await?;
    if report {
        let rest: FitnessIndex = index.into_iter().filter(|(id, _)| *id != stored).collect();
        ((deps.write_setting)(
            INDEX_KEY.to_string(),
            serde_json::to_value(&rest).map_err(|e| e.to_string())?,
        ))
        .await?;
    }
    Ok(ForgetOk {
        keys,
        models: model_rows(deps).await?,
        report,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The deps are wholesale, so the fixture half of this module is bigger than
// partial stubbing would make it: `panic_deps()` is the base (an unstubbed
// edge fails AT THE EDGE, naming itself, rather than quietly returning an
// empty answer), and each family of tests overwrites the edges it means to
// exercise.
//
// Two fixture facts:
//   — the settings store is a shared `Mutex<HashMap<String, Value>>` behind
//     the read/write edges, which is what `app_settings` is anyway;
//   — the cap test's gate is a `tokio::sync::watch`, which has the one
//     property the test needs: a waiter that registers late still sees the
//     release.
//
// THE RUN SLOTS ARE PROCESS-WIDE and `cargo test` runs this module on many
// threads in one process. Every test that can claim or clear a slot (Start,
// Stop, the cap) holds `RUN_SOLE` for its whole body so two tests never share
// the in-flight map — the pure tests run in parallel untouched.
#[cfg(test)]
mod tests {
    // The parent module's own `use` lines are private, so everything its code
    // spells unqualified is imported here too — glob only carries its pub items.
    use super::*;
    use crate::capability::CapabilityFact;
    use crate::fitness::adversarial::AdversarialEstimate;
    use crate::fitness::evals::{
        BandScores, EvalCaseScore, EvalSweep, EvalSweepState, HarnessMeta, HarnessScore,
        SweepConcurrency, TaskVerdict,
    };
    use crate::fitness::probes::{
        LatencyReading, ProbeEstimate, ProbeEstimateRow, ProbeId, ProbeReport,
    };
    use crate::fitness::score::FitnessReport;
    use crate::gateway::registry::{LlmEndpoint, ModelRouting};
    use crate::harness::define::{
        CheckCtx, CheckResult, EvalBand, EvalCase, HarnessDefinition, OnFailure, Output,
        RenderContext,
    };
    use crate::harness::registry::{HarnessSource, RegisteredHarness};
    use crate::harness::run::BoxFut;
    use crate::harness::schema::Schema;
    use crate::harness_model::ModelSpec;
    use crate::model::access::GatewayModel;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};

    static RUN_SOLE: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));
    async fn sole_runs() -> tokio::sync::MutexGuard<'static, ()> {
        RUN_SOLE.lock().await
    }

    // ── Fixtures ─────────────────────────────────────────────────────────────

    fn fact(value: bool) -> CapabilityFact {
        CapabilityFact {
            value,
            source: "probe".into(),
            at: "2026-08-01T00:00:00.000Z".into(),
            detail: None,
            score: None,
        }
    }

    /// Defaults plus metadata overrides, spelled out field by field — the
    /// overrides these tests care about are exactly the metadata ones.
    fn fact_over(
        value: bool,
        source: &str,
        detail: Option<&str>,
        score: Option<f64>,
        at: &str,
    ) -> CapabilityFact {
        CapabilityFact {
            value,
            source: source.into(),
            at: at.into(),
            detail: detail.map(String::from),
            score,
        }
    }

    /// `registry.rs` keeps registration private, so the shape is rebuilt the way
    /// score.rs's tests rebuild it: a `HarnessDefinition` leaked into the
    /// registry's slot. Built through `HarnessDefinition::new` rather than
    /// `define_harness` for the same reason score.rs gives — the derived json
    /// floor is a RUNTIME refusal the estimator never consults.
    fn json_harness(id: &'static str, fixtures: usize) -> RegisteredHarness {
        harness_of(id, fixtures, true)
    }

    /// A text harness — `run.rs` allows repairs only on JSON output, so one of
    /// these never gets a repair turn and budgeting one would inflate every
    /// estimate on a mostly-text registry. This fixture is what proves the
    /// estimate does not.
    fn text_harness(id: &'static str, fixtures: usize) -> RegisteredHarness {
        harness_of(id, fixtures, false)
    }

    fn harness_of(id: &'static str, fixtures: usize, json: bool) -> RegisteredHarness {
        let output = if json {
            Output::Json {
                schema: Schema::string(),
                preprocess: None,
                repair: None,
                verify: None,
            }
        } else {
            Output::Text {
                clean: None,
                verify: None,
            }
        };
        let mut def = HarnessDefinition::new(
            id,
            id,
            "test",
            ModelSpec {
                pin: None,
                role: None,
                chain: Some(&[]),
                user_id: None,
            },
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(Vec::new())),
            output,
            OnFailure::Null,
        );
        def.evals = (0..fixtures)
            .map(|_| {
                EvalCase::new(
                    "one",
                    json!({ "n": 1 }),
                    Arc::new(|_value: &Value, _ctx: &CheckCtx| CheckResult::Pass),
                )
            })
            .collect();
        RegisteredHarness {
            def: Box::leak(Box::new(def)),
            source: HarnessSource::Builtin,
        }
    }

    fn ep(name: &str) -> LlmEndpoint {
        LlmEndpoint {
            id: name.into(),
            name: name.into(),
            provider: "openai-compatible".into(),
            base_url: None,
            class: "local".into(),
            api_key_env: None,
            context_length: Some(32_000),
            models: Vec::new(),
            price_in_per_mtok: None,
            price_out_per_mtok: None,
            model_prices: Value::Null,
            auto_prices: Value::Null,
            request_defaults: Value::Null,
            model_efforts: Value::Null,
        }
    }

    fn probe_estimate() -> ProbeEstimate {
        ProbeEstimate {
            model: "m".into(),
            rows: Vec::new(),
            calls: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            usd: None,
            known: 0,
        }
    }

    fn adversarial_estimate() -> AdversarialEstimate {
        AdversarialEstimate {
            calls: 0,
            adversary_calls: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            worst_case: false,
        }
    }

    fn probe_report(wrote: i64) -> ProbeReport {
        ProbeReport {
            model: "m".into(),
            keys: Vec::new(),
            results: Vec::new(),
            wrote,
            latency: LatencyReading {
                requests: 0,
                errors: 0,
                p50: None,
                p95: None,
                usd: None,
            },
            ambiguous: None,
        }
    }

    /// Only the edges `estimate_run` actually reads; everything else stays the
    /// panicking base, which is the point — a test that drifts past the edges
    /// it meant to stub fails at the edge that drifted.
    fn estimate_deps() -> SurfaceDeps {
        let mut d = panic_deps();
        d.routing = Arc::new(|model| {
            Box::pin(async move {
                Ok(ModelRouting {
                    endpoints: Vec::new(),
                    upstream_model: model,
                })
            })
        });
        d.harnesses = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.read_setting = Arc::new(|_, fallback| Box::pin(async move { Ok(fallback) }));
        d.estimate_probes = Arc::new(|_, _| Box::pin(async { Ok(probe_estimate()) }));
        d.estimate_adversarial = Arc::new(|_, _| Box::pin(async { Ok(adversarial_estimate()) }));
        d
    }

    /// A read edge over a fixed map — the estimate's settings reads are
    /// read-only, so the store needs no write half.
    fn reading(
        pairs: Vec<(&str, Value)>,
    ) -> Arc<dyn Fn(String, Value) -> BoxFut<Result<Value, String>> + Send + Sync> {
        let map: HashMap<String, Value> =
            pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        Arc::new(move |key, fallback| {
            let v = map.get(&key).cloned().unwrap_or(fallback);
            Box::pin(async move { Ok(v) })
        })
    }

    type Store = Arc<Mutex<HashMap<String, Value>>>;

    fn store() -> Store {
        Arc::new(Mutex::new(HashMap::new()))
    }

    fn store_read(
        s: Store,
    ) -> Arc<dyn Fn(String, Value) -> BoxFut<Result<Value, String>> + Send + Sync> {
        Arc::new(move |key, fallback| {
            let v = s.lock().unwrap().get(&key).cloned().unwrap_or(fallback);
            Box::pin(async move { Ok(v) })
        })
    }

    fn store_write(
        s: Store,
    ) -> Arc<dyn Fn(String, Value) -> BoxFut<Result<(), String>> + Send + Sync> {
        Arc::new(move |key, value| {
            let s = s.clone();
            Box::pin(async move {
                s.lock().unwrap().insert(key, value);
                Ok(())
            })
        })
    }

    fn put(s: &Store, key: &str, value: Value) {
        s.lock().unwrap().insert(key.to_string(), value);
    }

    fn get(s: &Store, key: &str) -> Option<Value> {
        s.lock().unwrap().get(key).cloned()
    }

    /// Every field at its default so a test mutates only what its assertion
    /// is about.
    fn case(name: &str) -> EvalCaseScore {
        EvalCaseScore {
            harness: "h".into(),
            case: name.into(),
            band: EvalBand::Standard,
            skipped: None,
            contract_held: true,
            first_pass: true,
            repairs: 0,
            answered: true,
            task: TaskVerdict::Pass,
            task_error: None,
            gap: None,
            findings: 0,
            latency_ms: 10,
            started_at: "2026-08-01T00:00:00.000Z".into(),
            wall_ms: 0,
            prompt_tokens: 10,
            completion_tokens: 5,
            cost_usd: None,
            estimated: false,
            timed_out: false,
            optimistic: false,
            error: None,
            prompt: None,
            raw: None,
            turns: None,
            upstream: None,
            calls: None,
        }
    }

    fn entry(model: &str, at: &str) -> FitnessIndexEntry {
        FitnessIndexEntry {
            model: model.into(),
            at: at.into(),
            tiers: vec![TierId::Evals],
            guarded: true,
            cells: HashMap::new(),
            safety: None,
            probes_wrote: 0,
            speed: None,
            cost_usd: None,
            calls: 0,
            partial: false,
            harnesses: None,
        }
    }

    /// A harness score with nothing asserted about its rates, because the
    /// consumers under test only read the token and cost columns.
    fn hs(id: &str) -> HarnessScore {
        HarnessScore {
            meta: HarnessMeta {
                id: id.into(),
                label: "H".into(),
                source: "builtin".into(),
                output_kind: "json".into(),
                tools: "none".into(),
                requires: Vec::new(),
                verifies: false,
                repairable: true,
            },
            cases: 4,
            skipped: 0,
            gaps: 0,
            gap_reasons: Vec::new(),
            skip_reason: None,
            scored: 4,
            contract_rate: 1.0,
            repair_rate: 1.0,
            repair_yield: None,
            task_score: Some(1.0),
            band_scores: BandScores::default(),
            guard_rate: 0.0,
            answered_rate: 1.0,
            latency_p50: 0,
            latency_p95: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            estimated: false,
            timeouts: 0,
            optimistic: 0,
        }
    }

    fn sweep() -> EvalSweep {
        EvalSweep {
            model: "m".into(),
            state: EvalSweepState::Done,
            started_at: Some("2026-08-01T00:00:00.000Z".into()),
            finished_at: Some("2026-08-01T00:01:00.000Z".into()),
            done: 10,
            total: 10,
            error: None,
            harnesses: Vec::new(),
            cases: Vec::new(),
            unfixtured: Vec::new(),
            guarded: true,
            concurrency: SweepConcurrency {
                requested: 1,
                ended: 1,
                low: 1,
                narrowed_because: None,
            },
            measured: Vec::new(),
        }
    }

    fn report() -> FitnessReport {
        FitnessReport {
            model: "m".into(),
            slots: Vec::new(),
            unbound: Vec::new(),
            guarded: true,
        }
    }

    fn gm(id: &str, endpoints: &[&str], qualified: bool) -> GatewayModel {
        GatewayModel {
            id: id.into(),
            endpoints: endpoints.iter().map(|e| e.to_string()).collect(),
            qualified,
        }
    }

    /// Release the run slots a test claimed, on the real signal: the map is
    /// module state shared by every test in this file, so one that leaks turns
    /// the next test's `already-running` into `at-capacity`.
    async fn drain_runs() {
        for _ in 0..2_000 {
            if running_models().is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(
            running_models().is_empty(),
            "run slots leaked by a previous test"
        );
    }

    // ── mergeFact ────────────────────────────────────────────────────────────
    //
    // THE MOST CONSEQUENTIAL FUNCTION ON THE PAGE. It decides whether a
    // capability tag shows at all for a model id, which is to say it decides
    // what an admin believes about a model before they assign it a role. Both
    // directions of the unknown rule are asserted here, because "unknown is
    // not false" is what keeps a fresh self-host from rendering as a wall of
    // red — and the disagreement rule is what keeps a pooled id from being
    // credited with its best member's answer.

    #[test]
    fn merge_fact_reports_a_lone_measured_fact_verbatim() {
        let f = fact_over(
            true,
            "probe",
            Some("held json mode 3/3"),
            Some(1.0),
            "2026-07-04T12:00:00.000Z",
        );
        let v = merge_fact(&[Some(&f)]);
        assert_eq!(v.state, CapabilityState::Yes);
        assert_eq!(v.source.as_deref(), Some("probe"));
        assert_eq!(v.detail.as_deref(), Some("held json mode 3/3"));
        assert_eq!(v.score, Some(1.0));
        assert_eq!(v.at.as_deref(), Some("2026-07-04T12:00:00.000Z"));
        assert!(v.via.is_none());
    }

    #[test]
    fn merge_fact_reports_a_measured_false_as_no() {
        // A recorded failure is a fact, not a gap.
        let f = fact_over(
            false,
            "learned",
            Some("returned prose"),
            None,
            "2026-08-01T00:00:00.000Z",
        );
        let v = merge_fact(&[Some(&f)]);
        assert_eq!(v.state, CapabilityState::No);
        assert_eq!(v.source.as_deref(), Some("learned"));
        assert_eq!(v.detail.as_deref(), Some("returned prose"));
    }

    #[test]
    fn merge_fact_omits_absent_optionals_as_none() {
        let f = fact(true);
        let v = merge_fact(&[Some(&f)]);
        assert_eq!(v.state, CapabilityState::Yes);
        assert_eq!(v.source.as_deref(), Some("probe"));
        assert_eq!(v.detail, None);
        assert_eq!(v.score, None);
        assert_eq!(v.at.as_deref(), Some("2026-08-01T00:00:00.000Z"));
        assert!(v.via.is_none());
    }

    #[test]
    fn merge_fact_is_unknown_for_a_model_nothing_has_measured() {
        let unknown = FactView {
            state: CapabilityState::Unknown,
            source: None,
            detail: None,
            score: None,
            at: None,
            via: None,
        };
        let empty = merge_fact(&[]);
        let gap = merge_fact(&[None]);
        for v in [empty, gap] {
            assert_eq!(v.state, unknown.state);
            assert_eq!(v.source, unknown.source);
            assert_eq!(v.detail, unknown.detail);
            assert_eq!(v.score, unknown.score);
            assert_eq!(v.at, unknown.at);
            assert!(v.via.is_none());
        }
    }

    #[test]
    fn merge_fact_carries_an_agreeing_yes_pool_through() {
        let a = fact_over(
            true,
            "probe",
            Some("first"),
            None,
            "2026-08-01T00:00:00.000Z",
        );
        let b = fact_over(
            true,
            "probe",
            Some("second"),
            None,
            "2026-08-01T00:00:00.000Z",
        );
        let v = merge_fact(&[Some(&a), Some(&b)]);
        assert_eq!(v.state, CapabilityState::Yes);
        // The first member's metadata, deliberately: the alternative is
        // inventing a consensus sentence no endpoint actually wrote.
        assert_eq!(v.detail.as_deref(), Some("first"));
    }

    #[test]
    fn merge_fact_carries_an_agreeing_no_pool_through() {
        let a = fact(false);
        let b = fact(false);
        let c = fact(false);
        assert_eq!(
            merge_fact(&[Some(&a), Some(&b), Some(&c)]).state,
            CapabilityState::No
        );
    }

    #[test]
    fn merge_fact_is_unknown_when_the_pool_disagrees_in_either_direction() {
        let t = fact(true);
        let f = fact(false);
        let true_first = merge_fact(&[Some(&t), Some(&f)]);
        let false_first = merge_fact(&[Some(&f), Some(&t)]);
        // Order must not decide it. A vote — or "whichever member answered
        // first" — is exactly the false `true` that `run_probes` refuses to
        // write, because a bare id round-robins and the next call may land on
        // the other member.
        for v in [&true_first, &false_first] {
            assert_eq!(v.state, CapabilityState::Unknown);
            assert_eq!(v.detail.as_deref(), Some(POOLED_DISAGREEMENT));
            assert_eq!(v.source, None);
            assert_eq!(v.at, None);
        }
        assert_eq!(true_first.detail, false_first.detail);
        assert_eq!(true_first.source, false_first.source);
        assert_eq!(true_first.at, false_first.at);
    }

    #[test]
    fn merge_fact_is_unknown_when_one_member_was_never_measured_not_yes() {
        // A true from one endpoint says nothing about the endpoint nobody
        // probed. No disagreement sentence either: nothing disagreed, something
        // is simply missing, and telling an admin to "test the qualified id"
        // over a gap they can close by running the probes would be the wrong
        // instruction.
        let t = fact(true);
        let v = merge_fact(&[Some(&t), None]);
        assert_eq!(v.state, CapabilityState::Unknown);
        assert_eq!(v.detail, None);
    }

    #[test]
    fn merge_fact_is_unknown_when_one_member_was_never_measured_not_no() {
        // THE DIRECTION THAT MATTERS MOST. Collapsing a gap into `false` is how
        // every unprobed model on a fresh install turns red, and score.rs reads
        // a recorded `false` as UNFIT while an absent fact is merely untested.
        let f = fact(false);
        assert_eq!(
            merge_fact(&[Some(&f), None]).state,
            CapabilityState::Unknown
        );
        assert_eq!(
            merge_fact(&[None, Some(&f)]).state,
            CapabilityState::Unknown
        );
    }

    #[test]
    fn merge_fact_is_unknown_for_three_members_with_one_gap() {
        let t = fact(true);
        assert_eq!(
            merge_fact(&[Some(&t), Some(&t), None]).state,
            CapabilityState::Unknown
        );
    }

    // ── modelRows ────────────────────────────────────────────────────────────

    /// The modelRows deps: a catalog, a fact store, and nothing supplying
    /// anything (no MCP servers, no platform tools, no providers row) — so the
    /// states below are what was measured and nothing else.
    fn rows_deps() -> SurfaceDeps {
        let mut facts: HashMap<String, HashMap<String, CapabilityFact>> = HashMap::new();
        facts.insert(
            "spark:qwen3-14b".into(),
            HashMap::from([
                ("json".to_string(), fact(true)),
                ("tools".to_string(), fact(true)),
            ]),
        );
        facts.insert(
            "local:qwen3-14b".into(),
            HashMap::from([("json".to_string(), fact(false))]),
        );
        let mut d = panic_deps();
        let catalog = vec![
            gm("spark/qwen3-14b", &["spark"], true),
            gm("local/qwen3-14b", &["local"], true),
            gm("qwen3-14b", &["spark", "local"], false),
        ];
        d.models = Arc::new(move || {
            let c = catalog.clone();
            Box::pin(async move { Ok(c) })
        });
        d.capabilities = Arc::new(move |key| {
            let f = facts.get(&key).cloned().unwrap_or_default();
            Box::pin(async move { f })
        });
        d.mcp_servers = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.platform_supply = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.read_setting = Arc::new(|_, fallback| Box::pin(async move { Ok(fallback) }));
        d
    }

    #[tokio::test]
    async fn model_rows_flags_a_pooled_id_and_merges_its_facts() {
        let rows = model_rows(&rows_deps()).await.unwrap();
        let pooled = rows.iter().find(|r| r.id == "qwen3-14b").unwrap();
        assert!(pooled.pooled);
        let cap = |id: &str| pooled.capabilities.iter().find(|c| c.cap == id).unwrap();
        // json: measured on both and they disagree → unknown, with the sentence.
        let json = cap("json");
        assert_eq!(json.view.state, CapabilityState::Unknown);
        assert_eq!(json.view.detail.as_deref(), Some(POOLED_DISAGREEMENT));
        // tools: measured on one only → unknown, and NOT the `yes` that one
        // member would have given on its own.
        let tools = cap("tools");
        assert_eq!(tools.view.state, CapabilityState::Unknown);
        assert_eq!(tools.view.detail, None);
    }

    #[tokio::test]
    async fn model_rows_leaves_qualified_siblings_answering_for_themselves() {
        let rows = model_rows(&rows_deps()).await.unwrap();
        let state_of = |id: &str, cap: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap()
                .capabilities
                .iter()
                .find(|c| c.cap == cap)
                .unwrap()
                .view
                .state
        };
        assert_eq!(state_of("spark/qwen3-14b", "json"), CapabilityState::Yes);
        assert_eq!(state_of("local/qwen3-14b", "json"), CapabilityState::No);
        assert!(
            !rows
                .iter()
                .find(|r| r.id == "spark/qwen3-14b")
                .unwrap()
                .pooled
        );
    }

    #[test]
    fn keys_for_strips_the_endpoint_prefix_from_a_qualified_id_only() {
        assert_eq!(
            keys_for("spark/qwen3-14b", true, &["spark".to_string()]),
            vec!["spark:qwen3-14b".to_string()]
        );
        // A BARE id may itself contain a slash (OpenRouter names). The
        // `qualified` flag is the only thing that tells the two apart, so the
        // prefix must not be stripped here.
        assert_eq!(
            keys_for(
                "meta/llama-3.1-8b",
                false,
                &["spark".to_string(), "local".to_string()]
            ),
            vec![
                "spark:meta/llama-3.1-8b".to_string(),
                "local:meta/llama-3.1-8b".to_string()
            ]
        );
    }

    // ── The estimate ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn estimate_sums_a_probes_only_run_straight_off_estimate_probes() {
        // Three of nine probes will skip (no vision, no tools), and
        // `estimate_probes` already zeroes their calls. This asserts
        // `estimate_run` SUMS what it is handed rather than re-deriving the
        // skip rule — a second copy of that rule is how the estimate comes to
        // bill for calls the run never makes.
        let mut d = estimate_deps();
        let mut pe = probe_estimate();
        pe.rows = vec![
            ProbeEstimateRow {
                id: ProbeId::Json,
                calls: 3,
                prompt_tokens: 100,
                completion_tokens: 20,
                known: false,
            },
            ProbeEstimateRow {
                id: ProbeId::Vision,
                calls: 0,
                prompt_tokens: 500,
                completion_tokens: 40,
                known: false,
            },
            ProbeEstimateRow {
                id: ProbeId::Tools,
                calls: 0,
                prompt_tokens: 200,
                completion_tokens: 30,
                known: false,
            },
        ];
        pe.calls = 3;
        pe.prompt_tokens = 300;
        pe.completion_tokens = 60;
        pe.usd = Some(0.0042);
        d.estimate_probes = Arc::new(move |_, _| {
            let pe = pe.clone();
            Box::pin(async move { Ok(pe) })
        });
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Probes],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        assert_eq!(est.tiers.len(), 1);
        let t = &est.tiers[0];
        assert_eq!(t.tier, TierId::Probes);
        assert_eq!(t.calls, 3);
        assert_eq!(t.prompt_tokens, 300);
        assert_eq!(t.completion_tokens, 60);
        assert_eq!(t.usd, Some(0.0042));
        assert_eq!(t.basis, EstimateBasis::Fixture);
        assert_eq!(est.calls, 3);
        assert_eq!(est.usd, Some(0.0042));
        assert_eq!(est.unmeasured_harnesses, 0);
    }

    #[tokio::test]
    async fn estimate_reports_an_unsizeable_probe_suite_as_zero_and_voids_the_total() {
        let mut d = estimate_deps();
        d.estimate_probes = Arc::new(|_, _| Box::pin(async { Err("gateway down".to_string()) }));
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Probes],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        let t = &est.tiers[0];
        assert_eq!(
            (t.calls, t.prompt_tokens, t.completion_tokens, t.usd),
            (0, 0, 0, None)
        );
        // A dollar figure missing a component is a number nobody can reconcile
        // with the invoice, so there is no dollar figure.
        assert_eq!(est.usd, None);
        assert_eq!(est.calls, 0);
    }

    #[tokio::test]
    async fn estimate_bills_tier_two_one_repair_per_json_fixture_and_none_for_text() {
        let mut d = estimate_deps();
        d.harnesses = Arc::new(move || {
            Box::pin(async move {
                Ok(vec![
                    json_harness("j", 4),
                    text_harness("t", 6),
                    text_harness("empty", 0),
                ])
            })
        });
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Evals],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        // 10 fixtures + 4 repairs. A registry that budgeted a repair for the
        // six text fixtures would quote 20.
        assert_eq!(est.tiers[0].calls, 14);
        assert_eq!(est.fixtures, 10);
    }

    #[tokio::test]
    async fn estimate_prices_tier_two_off_the_measured_budget() {
        let mut d = estimate_deps();
        d.harnesses = Arc::new(move || {
            Box::pin(async move {
                Ok(vec![
                    json_harness("j", 4),
                    text_harness("t", 6),
                    text_harness("empty", 0),
                ])
            })
        });
        d.read_setting = reading(vec![(
            BUDGET_KEY,
            json!({ "j": { "prompt": 100, "completion": 25, "at": "2026-08-01T00:00:00.000Z" } }),
        )]);
        d.routing = Arc::new(|model| {
            Box::pin(async move {
                let mut e = ep("spark");
                e.price_in_per_mtok = Some(1.0);
                e.price_out_per_mtok = Some(4.0);
                Ok(ModelRouting {
                    endpoints: vec![e],
                    upstream_model: model,
                })
            })
        });
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Evals],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        let t = &est.tiers[0];
        assert_eq!((t.prompt_tokens, t.completion_tokens), (400, 100));
        assert_eq!(t.basis, EstimateBasis::Measured);
        // 400 * $1/MTok + 100 * $4/MTok.
        let want = (400.0 * 1.0 + 100.0 * 4.0) / 1e6;
        assert!((t.usd.unwrap() - want).abs() < 1e-12);
        // `t` has fixtures and no budget row; `empty` has no fixtures and is
        // not a gap — counting it would make the "figure is a floor" warning
        // permanent on a registry that will always have unfixtured harnesses.
        assert_eq!(est.unmeasured_harnesses, 1);
        assert!(t.note.contains("floor"));
        assert!(est.priced);
    }

    #[tokio::test]
    async fn estimate_says_nothing_about_a_floor_when_everything_was_measured() {
        let mut d = estimate_deps();
        d.harnesses = Arc::new(move || {
            Box::pin(async move { Ok(vec![json_harness("j", 2), text_harness("empty", 0)]) })
        });
        d.read_setting = reading(vec![(
            BUDGET_KEY,
            json!({ "j": { "prompt": 10, "completion": 5, "at": "2026-08-01T00:00:00.000Z" } }),
        )]);
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Evals],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        assert_eq!(est.unmeasured_harnesses, 0);
        assert!(!est.tiers[0].note.contains("floor"));
        // Nothing prices this model, so the tokens are exact and the dollars
        // absent.
        assert_eq!(est.tiers[0].usd, None);
        assert!(!est.priced);
    }

    #[tokio::test]
    async fn estimate_narrows_tier_two_to_the_harnesses_named_in_only() {
        let mut d = estimate_deps();
        d.harnesses = Arc::new(move || {
            Box::pin(async move { Ok(vec![json_harness("j", 3), text_harness("t", 9)]) })
        });
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Evals],
                adversary_model: None,
                only: Some(vec!["j".to_string()]),
            },
            &d,
        )
        .await
        .unwrap();
        assert_eq!(est.fixtures, 3);
        assert_eq!(est.tiers[0].calls, 6);
    }

    #[tokio::test]
    async fn estimate_counts_adversary_calls_and_prices_both_at_the_dearer_rate() {
        let quoted: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
        let mut d = estimate_deps();
        d.routing = Arc::new(|model| {
            Box::pin(async move {
                let mut e = if model == "dear" {
                    let mut e = ep("vendor");
                    e.price_in_per_mtok = Some(10.0);
                    e.price_out_per_mtok = Some(30.0);
                    e
                } else {
                    let mut e = ep("local");
                    e.price_in_per_mtok = Some(1.0);
                    e.price_out_per_mtok = Some(2.0);
                    e
                };
                e.models = vec![];
                Ok(ModelRouting {
                    endpoints: vec![e],
                    upstream_model: model,
                })
            })
        });
        let q = quoted.clone();
        d.estimate_adversarial = Arc::new(move |_, price| {
            let q = q.clone();
            Box::pin(async move {
                if let Some(p) = price {
                    *q.lock().unwrap() = p(1_000_000, 0).await;
                }
                Ok(AdversarialEstimate {
                    calls: 12,
                    adversary_calls: 12,
                    prompt_tokens: 5_000,
                    completion_tokens: 2_000,
                    cost_usd: Some(0.9),
                    worst_case: true,
                })
            })
        });
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "cheap".into(),
                tiers: vec![TierId::Adversarial],
                adversary_model: Some("dear".into()),
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        // Candidate + adversary, both counted: the run pays for both.
        assert_eq!(est.tiers[0].calls, 24);
        // Priced at the DEAR model's rate — a ceiling, never a surprise upward.
        let q = *quoted.lock().unwrap();
        assert!(q.is_some());
        assert!((q.unwrap() - 10.0).abs() < 1e-12);
        assert!(est.tiers[0].note.contains("ceiling"));
    }

    #[tokio::test]
    async fn estimate_drops_the_escalation_round_when_no_adversary_is_named() {
        let mut d = estimate_deps();
        d.estimate_adversarial = Arc::new(|_, _| {
            Box::pin(async {
                Ok(AdversarialEstimate {
                    calls: 12,
                    adversary_calls: 0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cost_usd: Some(0.1),
                    worst_case: false,
                })
            })
        });
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Adversarial],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        assert_eq!(est.tiers[0].calls, 12);
        assert!(est.tiers[0].note.contains("Naming an adversary"));
    }

    #[tokio::test]
    async fn estimate_adds_up_three_tiers_and_voids_the_total_when_one_cannot_price() {
        let mut d = estimate_deps();
        d.harnesses = Arc::new(move || Box::pin(async move { Ok(vec![json_harness("j", 2)]) }));
        d.read_setting = reading(vec![(
            BUDGET_KEY,
            json!({ "j": { "prompt": 100, "completion": 20, "at": "2026-08-01T00:00:00.000Z" } }),
        )]);
        d.routing = Arc::new(|model| {
            Box::pin(async move {
                let mut e = ep("spark");
                e.price_in_per_mtok = Some(2.0);
                e.price_out_per_mtok = Some(2.0);
                Ok(ModelRouting {
                    endpoints: vec![e],
                    upstream_model: model,
                })
            })
        });
        d.estimate_probes = Arc::new(|_, _| {
            Box::pin(async {
                let mut pe = probe_estimate();
                pe.calls = 9;
                pe.prompt_tokens = 900;
                pe.completion_tokens = 90;
                pe.usd = Some(0.001);
                Ok(pe)
            })
        });
        d.estimate_adversarial = Arc::new(|_, _| {
            Box::pin(async {
                Ok(AdversarialEstimate {
                    calls: 12,
                    adversary_calls: 0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cost_usd: Some(0.02),
                    worst_case: false,
                })
            })
        });
        let req = EstimateRequest {
            reprobe: false,
            model: "m".into(),
            tiers: vec![TierId::Probes, TierId::Evals, TierId::Adversarial],
            adversary_model: None,
            only: None,
        };
        let priced = estimate_run(&req, &d).await.unwrap();
        assert_eq!(
            priced.tiers.iter().map(|t| t.tier).collect::<Vec<_>>(),
            vec![TierId::Probes, TierId::Evals, TierId::Adversarial]
        );
        // 9 probe calls + (2 fixtures + 2 repairs) + 12 provocations.
        assert_eq!(priced.calls, 25);
        let evals_usd = (200.0 * 2.0 + 40.0 * 2.0) / 1e6;
        let want = 0.001 + evals_usd + 0.02;
        assert!((priced.usd.unwrap() - want).abs() < 1e-12);

        let mut un = d.clone();
        un.estimate_adversarial = Arc::new(|_, _| {
            Box::pin(async {
                Ok(AdversarialEstimate {
                    calls: 12,
                    adversary_calls: 0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cost_usd: None,
                    worst_case: false,
                })
            })
        });
        let unpriced = estimate_run(&req, &un).await.unwrap();
        assert_eq!(unpriced.calls, 25);
        assert_eq!(unpriced.usd, None);
    }

    #[tokio::test]
    async fn estimate_emits_no_row_for_a_tier_that_was_not_asked_for() {
        let mut d = estimate_deps();
        d.harnesses = Arc::new(move || Box::pin(async move { Ok(vec![json_harness("j", 1)]) }));
        d.estimate_probes = Arc::new(|_, _| Box::pin(async { Err("never called".to_string()) }));
        d.estimate_adversarial =
            Arc::new(|_, _| Box::pin(async { Err("never called".to_string()) }));
        let est = estimate_run(
            &EstimateRequest {
                reprobe: false,
                model: "m".into(),
                tiers: vec![TierId::Evals],
                adversary_model: None,
                only: None,
            },
            &d,
        )
        .await
        .unwrap();
        assert_eq!(
            est.tiers.iter().map(|t| t.tier).collect::<Vec<_>>(),
            vec![TierId::Evals]
        );
    }

    // ── Pricing ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn price_of_takes_the_dearest_endpoint_in_the_pool_not_the_average() {
        let mut d = panic_deps();
        d.routing = Arc::new(|model| {
            Box::pin(async move {
                let mut a = ep("a");
                a.price_in_per_mtok = Some(1.0);
                a.price_out_per_mtok = Some(1.0);
                let mut b = ep("b");
                b.price_in_per_mtok = Some(5.0);
                b.price_out_per_mtok = Some(9.0);
                Ok(ModelRouting {
                    endpoints: vec![a, b],
                    upstream_model: model,
                })
            })
        });
        // An estimate the round-robin can exceed is not an estimate an admin
        // can act on.
        assert_eq!(
            price_of("m", &d).await.unwrap(),
            Some(ModelPrice {
                in_per_mtok: 5.0,
                out_per_mtok: 9.0
            })
        );
    }

    #[tokio::test]
    async fn price_of_prefers_the_admin_override_over_the_auto_catalog_rate() {
        let mut d = panic_deps();
        d.routing = Arc::new(|_| {
            Box::pin(async move {
                let mut e = ep("a");
                e.model_prices = json!({ "up": { "in": 3, "out": 7 } });
                e.auto_prices = json!({ "up": { "in": 1, "out": 1 } });
                e.price_in_per_mtok = Some(99.0);
                e.price_out_per_mtok = Some(99.0);
                Ok(ModelRouting {
                    endpoints: vec![e],
                    upstream_model: "up".into(),
                })
            })
        });
        assert_eq!(
            price_of("m", &d).await.unwrap(),
            Some(ModelPrice {
                in_per_mtok: 3.0,
                out_per_mtok: 7.0
            })
        );
    }

    #[tokio::test]
    async fn price_of_is_none_when_nothing_serves_or_prices_the_model() {
        let mut none_serving = panic_deps();
        none_serving.routing = Arc::new(|model| {
            Box::pin(async move {
                Ok(ModelRouting {
                    endpoints: Vec::new(),
                    upstream_model: model,
                })
            })
        });
        assert_eq!(price_of("m", &none_serving).await.unwrap(), None);

        let mut unpriced = panic_deps();
        unpriced.routing = Arc::new(|model| {
            Box::pin(async move {
                Ok(ModelRouting {
                    endpoints: vec![ep("a")],
                    upstream_model: model,
                })
            })
        });
        assert_eq!(price_of("m", &unpriced).await.unwrap(), None);

        let mut no_catalog = panic_deps();
        no_catalog.routing = Arc::new(|_| Box::pin(async { Err("no catalog".to_string()) }));
        assert_eq!(price_of("m", &no_catalog).await.unwrap(), None);
    }

    #[test]
    fn usd_of_gives_no_dollars_without_a_price_rather_than_quoting_zero() {
        assert_eq!(usd_of(None, 1_000_000, 1_000_000), None);
        let p = Some(ModelPrice {
            in_per_mtok: 2.0,
            out_per_mtok: 6.0,
        });
        let usd = usd_of(p, 1_000_000, 500_000).unwrap();
        assert!((usd - 5.0).abs() < 1e-12);
    }

    // ── The drill-down ───────────────────────────────────────────────────────

    fn heavy(n: i64) -> EvalCaseScore {
        let mut c = case(&format!("fail-{n}"));
        c.contract_held = false;
        c.task = TaskVerdict::Fail;
        c.prompt = Some(format!("prompt {n}"));
        c.raw = Some(format!("reply {n}"));
        c.error = Some("schema rejected".into());
        c
    }

    fn clean(n: i64) -> EvalCaseScore {
        case(&format!("pass-{n}"))
    }

    #[test]
    fn drilldown_keeps_the_stored_prompt_and_reply_verbatim() {
        // That is what makes a red cell trustworthy.
        let (kept, dropped) = drilldown(&[clean(1), heavy(1), clean(2)], DRILLDOWN_CAP);
        assert_eq!(dropped, 0);
        let failed = kept.iter().find(|c| c.case == "fail-1").unwrap();
        assert_eq!(failed.prompt.as_deref(), Some("prompt 1"));
        assert_eq!(failed.raw.as_deref(), Some("reply 1"));
        assert_eq!(failed.error.as_deref(), Some("schema rejected"));
    }

    #[test]
    fn drilldown_keeps_a_case_with_only_one_half_of_the_transcript() {
        // `run.rs` can record a prompt with no reply (the transport died) or a
        // reply with no prompt. Either is a transcript and either is heavy.
        let mut prompt_only = case("p");
        prompt_only.prompt = Some("asked".into());
        let mut reply_only = case("r");
        reply_only.raw = Some("answered".into());
        let (kept, _) = drilldown(&[prompt_only, reply_only], DRILLDOWN_CAP);
        assert_eq!(
            kept.iter().map(|c| c.case.as_str()).collect::<Vec<_>>(),
            vec!["p", "r"]
        );
    }

    #[test]
    fn drilldown_keeps_every_clean_case_whole_even_past_the_cap() {
        // The cap is on transcripts.
        let mut cases: Vec<EvalCaseScore> = (0..40).map(clean).collect();
        cases.extend((0..5).map(heavy));
        let (kept, dropped) = drilldown(&cases, 3);
        assert_eq!(dropped, 2);
        // 3 transcripts + all 40 clean rows: dropping the cheap ones would
        // leave the panel unable to say how many fixtures actually passed.
        assert_eq!(kept.len(), 43);
        assert_eq!(
            kept.iter()
                .filter(|c| c.prompt.is_some() || c.raw.is_some())
                .count(),
            3
        );
    }

    #[test]
    fn drilldown_drops_nothing_at_the_cap_and_one_at_the_cap_plus_one() {
        let at_cap: Vec<EvalCaseScore> = (0..30).map(heavy).collect();
        let (kept, dropped) = drilldown(&at_cap, DRILLDOWN_CAP);
        assert_eq!(dropped, 0);
        assert_eq!(kept.len(), 30);

        let mut over_cap = at_cap.clone();
        over_cap.push(heavy(30));
        let (over_kept, over_dropped) = drilldown(&over_cap, DRILLDOWN_CAP);
        assert_eq!(over_dropped, 1);
        assert_eq!(over_kept.len(), 30);
        // The newest transcript is the one that falls off — the take keeps the
        // head, which is sweep order.
        assert!(!over_kept.iter().any(|c| c.case == "fail-30"));
    }

    #[test]
    fn drilldown_reports_no_drop_for_an_empty_sweep() {
        let (kept, dropped) = drilldown(&[], DRILLDOWN_CAP);
        assert!(kept.is_empty());
        assert_eq!(dropped, 0);
    }

    // ── The archive ──────────────────────────────────────────────────────────

    fn index_of(n: usize) -> FitnessIndex {
        let mut out = FitnessIndex::new();
        // Model 0 is the OLDEST. Dates are ISO so the sort is lexicographic.
        for i in 0..n {
            let at = format!("2026-08-{:02}T00:00:00.000Z", i + 1);
            out.insert(format!("m{i}"), entry(&format!("m{i}"), &at));
        }
        out
    }

    #[test]
    fn evict_archive_evicts_nothing_below_the_cap() {
        let (index, evicted) = evict_archive(&index_of(23), 24);
        assert!(evicted.is_empty());
        assert_eq!(index.len(), 23);
    }

    #[test]
    fn evict_archive_evicts_nothing_at_exactly_the_cap() {
        let (index, evicted) = evict_archive(&index_of(24), 24);
        assert!(evicted.is_empty());
        assert_eq!(index.len(), 24);
    }

    #[test]
    fn evict_archive_evicts_the_single_oldest_at_the_cap_plus_one() {
        let (index, evicted) = evict_archive(&index_of(25), 24);
        assert_eq!(evicted, vec!["m0"]);
        assert_eq!(index.len(), 24);
        assert!(!index.contains_key("m0"));
        assert!(index.contains_key("m24"));
    }

    #[test]
    fn evict_archive_evicts_oldest_first_when_well_over_the_cap() {
        let (index, evicted) = evict_archive(&index_of(30), 24);
        assert_eq!(evicted, vec!["m5", "m4", "m3", "m2", "m1", "m0"]);
        assert_eq!(index.len(), 24);
    }

    #[test]
    fn evict_archive_leaves_the_index_it_was_handed_untouched() {
        let before = index_of(26);
        let _ = evict_archive(&before, 24);
        // The index and the report rows are written together; a mutation that
        // landed before a failed write would leave the matrix listing models
        // whose reports had already been deleted.
        assert_eq!(before.len(), 26);
    }

    #[test]
    fn evict_archive_defaults_to_the_shipped_cap() {
        let (_, evicted) = evict_archive(&index_of(25), KEEP_MODELS);
        assert_eq!(evicted, vec!["m0"]);
    }

    // ── Start rejections ─────────────────────────────────────────────────────
    //
    // Every one of these returns BEFORE the run slot is claimed and before a
    // single model call is bought, so none of them starts anything.

    fn reject_deps() -> SurfaceDeps {
        let mut d = panic_deps();
        let catalog = vec![
            gm("spark/qwen3-14b", &["spark"], true),
            gm("spark/gpt-5", &["spark"], true),
        ];
        d.models = Arc::new(move || {
            let c = catalog.clone();
            Box::pin(async move { Ok(c) })
        });
        d.capabilities = Arc::new(|_| Box::pin(async { HashMap::new() }));
        d.mcp_servers = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.platform_supply = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.read_setting = Arc::new(|_, fallback| Box::pin(async move { Ok(fallback) }));
        d
    }

    fn start_req(model: &str, tiers: &[TierId], adversary: Option<&str>) -> StartRequest {
        StartRequest {
            model: model.into(),
            tiers: tiers.to_vec(),
            adversary_model: adversary.map(String::from),
            only: None,
            restart: false,
            reprobe: false,
            concurrency: None,
            retry_failed: false,
            supplement: false,
        }
    }

    #[tokio::test]
    async fn start_refuses_a_candidate_the_gateway_does_not_serve() {
        let _sole = sole_runs().await;
        let out = start_fitness_run(
            start_req("not-a-model", &[TierId::Probes], None),
            Arc::new(reject_deps()),
        )
        .await;
        match out {
            StartOutcome::Rejected(e) => assert_eq!(e, "that model is not on the gateway"),
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn start_refuses_an_adversary_the_gateway_does_not_serve() {
        let _sole = sole_runs().await;
        let out = start_fitness_run(
            start_req("spark/qwen3-14b", &[TierId::Probes], Some("ghost")),
            Arc::new(reject_deps()),
        )
        .await;
        match out {
            StartOutcome::Rejected(e) => {
                assert_eq!(e, "that adversary model is not on the gateway")
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn start_refuses_to_let_a_model_grade_its_own_resistance() {
        let _sole = sole_runs().await;
        // The who-judges-the-judge regress with the stakes turned up: a model
        // asked to break itself scores itself safe, and the number goes on the
        // page.
        let out = start_fitness_run(
            start_req("spark/gpt-5", &[TierId::Adversarial], Some("spark/gpt-5")),
            Arc::new(reject_deps()),
        )
        .await;
        match out {
            StartOutcome::Rejected(e) => {
                assert_eq!(
                    e,
                    "the adversary must be a different model than the candidate"
                )
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    // ── forgetModel ──────────────────────────────────────────────────────────

    fn forget_deps(forgotten: Arc<Mutex<Vec<String>>>, s: Store) -> SurfaceDeps {
        let mut d = panic_deps();
        let catalog = vec![gm("qwen3-14b", &["spark", "local"], false)];
        d.models = Arc::new(move || {
            let c = catalog.clone();
            Box::pin(async move { Ok(c) })
        });
        d.capabilities = Arc::new(|_| Box::pin(async { HashMap::new() }));
        d.mcp_servers = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.platform_supply = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.forget = Arc::new(move |key| {
            let f = forgotten.clone();
            Box::pin(async move {
                f.lock().unwrap().push(key);
                Ok(())
            })
        });
        d.read_setting = store_read(s.clone());
        d.write_setting = store_write(s);
        d
    }

    #[tokio::test]
    async fn forget_clears_every_endpoint_key_a_bare_id_could_be_served_from() {
        let forgotten: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let out = forget_model("qwen3-14b", &forget_deps(forgotten.clone(), store()))
            .await
            .unwrap();
        // Per endpoint:model rather than per id — the release valve on the
        // gateway's one-way ratchet has to reach every fact the id has.
        assert_eq!(
            *forgotten.lock().unwrap(),
            vec!["spark:qwen3-14b".to_string(), "local:qwen3-14b".to_string()]
        );
        assert_eq!(
            out.keys,
            vec!["spark:qwen3-14b".to_string(), "local:qwen3-14b".to_string()]
        );
    }

    #[tokio::test]
    async fn forget_deletes_the_archived_report_and_its_index_entry_too() {
        // THE REASON THE BUTTON LOOKED BROKEN. Talaria records what it knows
        // about a model in two places and this cleared one, so an admin pressed
        // Forget, the panel refetched, and every probe verdict they had just
        // been told was deleted was still on the screen.
        let s = store();
        let mut index = FitnessIndex::new();
        index.insert("qwen3-14b".into(), entry("qwen3-14b", "x"));
        index.insert("other-model".into(), entry("other-model", "y"));
        put(
            &s,
            &record_key("qwen3-14b"),
            json!({ "model": "qwen3-14b" }),
        );
        put(&s, INDEX_KEY, serde_json::to_value(&index).unwrap());

        let out = forget_model(
            "qwen3-14b",
            &forget_deps(Arc::new(Mutex::new(Vec::new())), s.clone()),
        )
        .await
        .unwrap();
        assert!(out.report);
        assert_eq!(get(&s, &record_key("qwen3-14b")), Some(Value::Null));
        // Only this model leaves the index; every other verdict on the page
        // stays.
        let rest = get(&s, INDEX_KEY).unwrap();
        let keys: Vec<&str> = rest
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["other-model"]);
    }

    #[tokio::test]
    async fn forget_is_idempotent_on_a_model_nobody_has_swept() {
        let forgotten: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let out = forget_model("qwen3-14b", &forget_deps(forgotten.clone(), store()))
            .await
            .unwrap();
        // The facts still go; there was simply no report to go with them,
        // which is a thing to report rather than a thing to fail on.
        assert!(!out.report);
        assert_eq!(
            *forgotten.lock().unwrap(),
            vec!["spark:qwen3-14b".to_string(), "local:qwen3-14b".to_string()]
        );
    }

    #[tokio::test]
    async fn forget_refuses_an_id_the_gateway_does_not_serve_and_forgets_nothing() {
        let forgotten: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let err = forget_model("ghost", &forget_deps(forgotten.clone(), store()))
            .await
            .unwrap_err();
        assert_eq!(err, "that model is not on the gateway");
        assert!(forgotten.lock().unwrap().is_empty());
    }

    // ── indexEntryOf ─────────────────────────────────────────────────────────

    #[test]
    fn index_entry_is_not_partial_when_every_requested_tier_produced_something() {
        let rep = report();
        let sw = sweep();
        let pr = probe_report(4);
        let e = index_entry_of(IndexEntryParts {
            model: "m",
            at: "2026-08-06T00:00:00.000Z",
            ran: &[TierId::Probes, TierId::Evals],
            requested: &[TierId::Probes, TierId::Evals],
            sweep: &sw,
            report: &rep,
            probes: Some(&pr),
            adversarial: None,
            previous_speed: None,
        });
        assert!(!e.partial);
        assert_eq!(e.probes_wrote, 4);
    }

    #[test]
    fn index_entry_is_partial_when_a_requested_tier_produced_nothing() {
        // The archived record is stamped with the tiers that RAN. A record
        // claiming a tier that never happened is the same lie as a green cell
        // nobody filled.
        let rep = report();
        let sw = sweep();
        let e = index_entry_of(IndexEntryParts {
            model: "m",
            at: "2026-08-06T00:00:00.000Z",
            ran: &[TierId::Evals],
            requested: &[TierId::Probes, TierId::Evals],
            sweep: &sw,
            report: &rep,
            probes: None,
            adversarial: None,
            previous_speed: None,
        });
        assert!(e.partial);
        assert_eq!(e.tiers, vec![TierId::Evals]);
        assert_eq!(e.probes_wrote, 0);
    }

    #[test]
    fn index_entry_is_partial_when_the_sweep_stopped_mid_run() {
        let rep = report();
        let mut sw = sweep();
        sw.state = EvalSweepState::Stopped;
        sw.done = 3;
        let e = index_entry_of(IndexEntryParts {
            model: "m",
            at: "2026-08-06T00:00:00.000Z",
            ran: &[TierId::Evals],
            requested: &[TierId::Evals],
            sweep: &sw,
            report: &rep,
            probes: None,
            adversarial: None,
            previous_speed: None,
        });
        assert!(e.partial);
    }

    #[test]
    fn index_entry_does_not_call_a_fully_priced_run_unpriced_over_one_skipped_harness() {
        // WHAT MADE THIS SHOW UP. A harness whose cases were all skipped
        // reports `cost_usd: null` — it priced nothing because it SPENT
        // nothing. Treating that as "unpriced" turned every run with a skip
        // into a dash in the modal header, and the qwen run (a routing refusal
        // skipped everything) made it impossible to miss.
        let rep = report();
        let mut a = hs("a");
        a.cost_usd = Some(0.02);
        a.prompt_tokens = 900;
        a.completion_tokens = 120;
        let mut b = hs("b");
        b.cost_usd = Some(0.01);
        b.prompt_tokens = 400;
        b.completion_tokens = 80;
        let mut c = hs("c");
        c.cost_usd = None;
        c.prompt_tokens = 0;
        c.completion_tokens = 0;
        c.cases = 0;
        c.skipped = 3;
        let mut sw = sweep();
        sw.harnesses = vec![a, b, c];
        let e = index_entry_of(IndexEntryParts {
            model: "m",
            at: "2026-08-06T00:00:00.000Z",
            ran: &[TierId::Evals],
            requested: &[TierId::Evals],
            sweep: &sw,
            report: &rep,
            probes: None,
            adversarial: None,
            previous_speed: None,
        });
        assert!((e.cost_usd.unwrap() - 0.03).abs() < 1e-12);
    }

    #[test]
    fn index_entry_still_refuses_a_total_when_something_that_burned_tokens_went_unpriced() {
        // The case the all-or-nothing rule was written for: a partial total
        // under a dollar sign is a number nobody can reconcile with the
        // invoice.
        let rep = report();
        let mut a = hs("a");
        a.cost_usd = Some(0.02);
        a.prompt_tokens = 900;
        a.completion_tokens = 120;
        let mut b = hs("b");
        b.cost_usd = None;
        b.prompt_tokens = 400;
        b.completion_tokens = 80;
        let mut sw = sweep();
        sw.harnesses = vec![a, b];
        let e = index_entry_of(IndexEntryParts {
            model: "m",
            at: "2026-08-06T00:00:00.000Z",
            ran: &[TierId::Evals],
            requested: &[TierId::Evals],
            sweep: &sw,
            report: &rep,
            probes: None,
            adversarial: None,
            previous_speed: None,
        });
        assert_eq!(e.cost_usd, None);
    }

    // ── The token budget a sweep leaves behind ───────────────────────────────

    fn budget_score(id: &str) -> HarnessScore {
        let mut h = hs(id);
        h.cases = 4;
        h.prompt_tokens = 4000;
        h.completion_tokens = 800;
        h
    }

    fn good_budget() -> TokenBudget {
        vec![(
            "titler".to_string(),
            TokenBudgetEntry {
                prompt: 103,
                completion: 11,
                at: "yesterday".into(),
            },
        )]
        .into_iter()
        .collect()
    }

    #[test]
    fn next_budget_records_per_case_tokens_for_what_a_run_actually_measured() {
        let want: TokenBudget = vec![(
            "titler".to_string(),
            TokenBudgetEntry {
                prompt: 1000,
                completion: 200,
                at: "now".into(),
            },
        )]
        .into_iter()
        .collect();
        assert_eq!(
            next_budget(&TokenBudget::new(), &[budget_score("titler")], "now"),
            want
        );
    }

    #[test]
    fn next_budget_does_not_let_a_run_that_measured_nothing_overwrite_one_that_did() {
        // THE BUG THIS LOCKS. A sweep against a model id the gateway could not
        // reach ran every case, failed every one before a token moved, and
        // wrote 0/0 across the registry. Both dollar figures downstream then
        // read $0.00 for every model — a confident number nobody could
        // reconcile.
        let zero = {
            let mut h = budget_score("titler");
            h.prompt_tokens = 0;
            h.completion_tokens = 0;
            h
        };
        assert_eq!(next_budget(&good_budget(), &[zero], "now"), good_budget());
    }

    #[test]
    fn next_budget_still_skips_a_harness_the_sweep_never_ran_a_case_of() {
        let none = {
            let mut h = budget_score("titler");
            h.cases = 0;
            h.prompt_tokens = 0;
            h.completion_tokens = 0;
            h
        };
        assert_eq!(next_budget(&good_budget(), &[none], "now"), good_budget());
    }

    #[test]
    fn next_budget_takes_the_measured_harnesses_and_leaves_the_unmeasured_ones_alone() {
        let zero = {
            let mut h = budget_score("titler");
            h.prompt_tokens = 0;
            h.completion_tokens = 0;
            h
        };
        let after = next_budget(&good_budget(), &[zero, budget_score("summarizer")], "now");
        assert_eq!(after.get("titler"), good_budget().get("titler"));
        assert_eq!(
            after.get("summarizer"),
            Some(&TokenBudgetEntry {
                prompt: 1000,
                completion: 200,
                at: "now".into()
            })
        );
    }

    // ── The archive, re-keyed onto the ids the catalog offers ────────────────

    fn rekey_catalog() -> Vec<GatewayModel> {
        vec![
            gm(
                "openrouter/deepseek/deepseek-v4-flash",
                &["openrouter"],
                true,
            ),
            gm("spark-a/qwen3-14b", &["spark-a"], true),
            gm("spark-b/qwen3-14b", &["spark-b"], true),
            gm("qwen3-14b", &["spark-a", "spark-b"], false),
        ]
    }

    fn index_from(pairs: Vec<(&str, &str)>) -> FitnessIndex {
        pairs
            .into_iter()
            .map(|(model, at)| (model.to_string(), entry(model, at)))
            .collect()
    }

    #[test]
    fn the_archive_lights_the_canonical_row_from_a_report_archived_under_the_bare_id() {
        // The run was paid for. Left keyed bare, its verdicts colour no cell
        // and the page asks the admin to buy it again.
        let out = canonical_index(
            &index_from(vec![("deepseek/deepseek-v4-flash", "monday")]),
            &rekey_catalog(),
        );
        let keys: Vec<&String> = out.keys().collect();
        assert_eq!(
            keys,
            vec![&"openrouter/deepseek/deepseek-v4-flash".to_string()]
        );
        // THE KEY MOVES, THE STORED SPELLING DOES NOT. `model` is what
        // `record_key` needs, and overwriting it with the canonical id is what
        // broke the drill-down and the value view's backfill: both went
        // looking for `model_fitness_report:openrouter/deepseek/…` when the
        // archive is filed under `model_fitness_report:deepseek/…`.
        assert_eq!(
            out["openrouter/deepseek/deepseek-v4-flash"].model,
            "deepseek/deepseek-v4-flash"
        );
        assert_eq!(
            stored_id_for("openrouter/deepseek/deepseek-v4-flash", &out),
            "deepseek/deepseek-v4-flash"
        );
    }

    #[test]
    fn the_archive_leaves_an_id_that_never_moved_alone() {
        let out = canonical_index(
            &index_from(vec![("spark-a/qwen3-14b", "monday")]),
            &rekey_catalog(),
        );
        assert_eq!(
            stored_id_for("spark-a/qwen3-14b", &out),
            "spark-a/qwen3-14b"
        );
        // And a model with no entry at all is its own stored id.
        assert_eq!(stored_id_for("never-tested", &out), "never-tested");
    }

    #[test]
    fn the_archive_never_displaces_an_entry_already_stored_under_its_canonical_id() {
        let out = canonical_index(
            &index_from(vec![
                ("deepseek/deepseek-v4-flash", "monday"),
                ("openrouter/deepseek/deepseek-v4-flash", "friday"),
            ]),
            &rekey_catalog(),
        );
        assert_eq!(out["openrouter/deepseek/deepseek-v4-flash"].at, "friday");
    }

    #[test]
    fn the_archive_leaves_a_pooled_id_where_it_is() {
        // `qwen3-14b` is a round-robin target in its own right, not a
        // misspelling of either endpoint's pin.
        let out = canonical_index(&index_from(vec![("qwen3-14b", "monday")]), &rekey_catalog());
        let keys: Vec<&String> = out.keys().collect();
        assert_eq!(keys, vec![&"qwen3-14b".to_string()]);
    }

    // ── Testing several candidates at once ───────────────────────────────────
    //
    // A REAL SLOT IS CLAIMED by every start that gets past validation, and this
    // process has no way to un-start one — so each case here stops what it
    // started. The runs are held open by a gate on `run_probes`, so no model
    // call is ever bought and no slot is ever released early.

    /// Every edge a detached probes-only run touches on its way to Done. The
    /// empty answers the real (test-empty) registry would have given are said
    /// out loud, edge by edge.
    fn run_deps(
        s: &Store,
        gate: &tokio::sync::watch::Receiver<bool>,
        now: &'static str,
    ) -> SurfaceDeps {
        let mut catalog = Vec::new();
        for i in 0..MAX_CONCURRENT_RUNS {
            catalog.push(gm(&format!("spark/m{i}"), &["spark"], true));
        }
        catalog.push(gm("spark/over", &["spark"], true));
        catalog.push(gm("spark/a", &["spark"], true));
        let mut d = panic_deps();
        d.models = Arc::new(move || {
            let c = catalog.clone();
            Box::pin(async move { Ok(c) })
        });
        d.capabilities = Arc::new(|_| Box::pin(async { HashMap::new() }));
        d.mcp_servers = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.platform_supply = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.read_setting = store_read(s.clone());
        d.write_setting = store_write(s.clone());
        d.eval_sweep_statuses = Arc::new(|_| Box::pin(async { Ok(HashMap::new()) }));
        d.stop_eval_sweep = Arc::new(|_| false);
        d.harnesses = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.guard_config = Arc::new(|| Box::pin(async { Err("no guard in tests".to_string()) }));
        d.observed_harnesses = Arc::new(|_| Box::pin(async { Ok(Vec::new()) }));
        d.reach = Arc::new(|_, _| Box::pin(async { Ok(HashMap::new()) }));
        d.bind_slots = Arc::new(|_| Box::pin(async { Ok(Vec::new()) }));
        d.now_iso = Arc::new(move || now.to_string());
        // HELD OPEN ON PURPOSE. Every tier is stubbed, so without a gate a
        // detached run finishes before the next Start is issued and releases
        // its slot — the cap would never be reached and the test would pass
        // for the wrong reason.
        let rx = gate.clone();
        d.run_probes = Arc::new(move |_, _| {
            let mut rx = rx.clone();
            Box::pin(async move {
                while !*rx.borrow_and_update() {
                    rx.changed().await.unwrap();
                }
                Ok(probe_report(0))
            })
        });
        d
    }

    async fn start(model: &str, d: &SurfaceDeps) -> StartOutcome {
        start_fitness_run(
            start_req(model, &[TierId::Probes], None),
            Arc::new(d.clone()),
        )
        .await
    }

    #[tokio::test]
    async fn runs_candidates_side_by_side_up_to_the_cap_and_refuses_the_one_past_it() {
        let _sole = sole_runs().await;
        let (open_tx, gate) = tokio::sync::watch::channel(false);
        let s = store();
        let d = run_deps(&s, &gate, NOW);
        for i in 0..MAX_CONCURRENT_RUNS {
            let out = start(&format!("spark/m{i}"), &d).await;
            assert!(
                matches!(out, StartOutcome::Started { .. }),
                "start {i} was refused"
            );
        }

        let overflow = start("spark/over", &d).await;
        assert!(matches!(
            overflow,
            StartOutcome::Busy {
                refusal: RunRefusal::AtCapacity,
                ..
            }
        ));
        assert!(fitness_runs(&d).await.unwrap().full);

        let _ = open_tx.send(true);
        stop_fitness_run(None, &d).await;
        drain_runs().await;
    }

    #[tokio::test]
    async fn names_the_refusal_so_the_route_can_say_which_wall_was_hit() {
        // "busy" alone had one meaning when one run was the maximum. With
        // eight slots, "you already started this one" and "every slot is
        // taken" are different sentences and different fixes.
        let _sole = sole_runs().await;
        let (open_tx, gate) = tokio::sync::watch::channel(false);
        let s = store();
        let d = run_deps(&s, &gate, NOW);
        assert!(matches!(
            start("spark/a", &d).await,
            StartOutcome::Started { .. }
        ));
        let again = start("spark/a", &d).await;
        assert!(matches!(
            again,
            StartOutcome::Busy {
                refusal: RunRefusal::AlreadyRunning,
                ..
            }
        ));

        let _ = open_tx.send(true);
        stop_fitness_run(None, &d).await;
        drain_runs().await;
    }

    #[tokio::test]
    async fn reports_the_cap_so_the_page_never_has_to_restate_it() {
        let s = store();
        let mut d = panic_deps();
        d.read_setting = store_read(s.clone());
        d.eval_sweep_statuses = Arc::new(|_| Box::pin(async { Ok(HashMap::new()) }));
        d.now_iso = Arc::new(|| NOW.to_string());
        let view = fitness_runs(&d).await.unwrap();
        assert_eq!(view.max, MAX_CONCURRENT_RUNS);
        // Raised from 3 once the checkpoint stopped being one shared row — see
        // the note on the constant. Held here so a change is deliberate.
        assert_eq!(MAX_CONCURRENT_RUNS, 8);
    }

    /// A persisted status row, as raw JSON — the shape the archive and another
    /// instance's writes actually leave in the settings store.
    fn row(state: &str, model: &str, started_at: &str, heartbeat_at: Option<&str>) -> Value {
        let mut v = json!({
            "state": state,
            "model": model,
            "tiers": ["evals"],
            "phase": "evals",
            "startedAt": started_at,
        });
        if let Some(beat) = heartbeat_at {
            v["heartbeatAt"] = json!(beat);
        }
        v
    }

    fn view_deps(s: &Store) -> SurfaceDeps {
        let mut d = panic_deps();
        d.read_setting = store_read(s.clone());
        d.eval_sweep_statuses = Arc::new(|_| Box::pin(async { Ok(HashMap::new()) }));
        d.stop_eval_sweep = Arc::new(|_| false);
        d.models = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.now_iso = Arc::new(|| NOW.to_string());
        d
    }

    // The wire rows are raw Values (key order is the contract — see
    // `fitness_runs`), so the tests read them by field.
    fn view_models(view: &FitnessRunsView) -> Vec<String> {
        view.runs
            .iter()
            .map(|r| {
                r.get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn folds_the_legacy_single_status_row_in_under_its_own_model() {
        // The row a run already in flight keeps writing across this change.
        // Not reading it means an admin watches a run start and never sees it
        // finish.
        let s = store();
        put(
            &s,
            STATUS_KEY,
            row("running", "spark/legacy", "monday", None),
        );
        let view = fitness_runs(&view_deps(&s)).await.unwrap();
        assert_eq!(view_models(&view), vec!["spark/legacy"]);
    }

    #[tokio::test]
    async fn lets_a_real_entry_win_over_the_legacy_row_for_the_same_model() {
        let s = store();
        put(&s, STATUS_KEY, row("running", "spark/a", "monday", None));
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/a": row("done", "spark/a", "friday", None) }),
        );
        let view = fitness_runs(&view_deps(&s)).await.unwrap();
        assert_eq!(view.runs.len(), 1);
        assert_eq!(
            view.runs[0].get("state").and_then(Value::as_str),
            Some("done")
        );
    }

    #[tokio::test]
    async fn puts_the_running_rows_first_then_the_most_recently_started() {
        // `heartbeatAt: now` on the live row: this test is about ORDERING, and
        // without a heartbeat a 2026-01-02 row is (correctly) months stale and
        // reports as failed. See `stale_run`.
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({
                "old": row("done", "old", "2026-01-01", None),
                "live": row("running", "live", "2026-01-02", Some(NOW)),
                "recent": row("done", "recent", "2026-01-03", None),
            }),
        );
        let view = fitness_runs(&view_deps(&s)).await.unwrap();
        assert_eq!(view_models(&view), vec!["live", "recent", "old"]);
    }

    // ── The live case list ───────────────────────────────────────────────────

    #[test]
    fn the_live_list_keeps_everything_that_failed_and_only_a_few_of_what_passed() {
        // Polled every three seconds. A clean case says nothing the progress
        // counter does not already say, and sending 155 of them per poll —
        // growing to 247 — is the whole sweep on the wire twenty times a
        // minute.
        let mut cases: Vec<EvalCaseScore> = (0..100).map(|i| case(&format!("clean-{i}"))).collect();
        let mut failed = case("failed");
        failed.task = TaskVerdict::Fail;
        let mut no_contract = case("no-contract");
        no_contract.contract_held = false;
        let mut slow = case("slow");
        slow.timed_out = true;
        cases.extend([failed, no_contract, slow]);
        let (kept, dropped) = live_cases(&cases);
        let bad =
            |c: &EvalCaseScore| c.task == TaskVerdict::Fail || !c.contract_held || c.timed_out;
        assert_eq!(kept.iter().filter(|c| bad(c)).count(), 3);
        assert!(kept.len() < 12);
        assert!(dropped > 90);
    }

    #[test]
    fn the_live_list_never_counts_a_skipped_case_as_a_failure() {
        // A skip is an absence, not a zero — the distinction the whole scoring
        // layer turns on, and it must not be undone by the view that shows it.
        // A skipped case carries `contractHeld: false`, so a filter that
        // forgot to check `skipped` would retain every one of them as a
        // failure forever.
        let mut skip = case("skip");
        skip.skipped = Some("this candidate runs no tool loop".into());
        skip.contract_held = false;
        skip.task = TaskVerdict::Unscored;
        let cases: Vec<EvalCaseScore> = std::iter::once(skip)
            .chain((0..20).map(|i| case(&format!("clean-{i}"))))
            .collect();
        let (kept, _) = live_cases(&cases);
        assert_eq!(kept.iter().filter(|c| c.skipped.is_some()).count(), 0);
    }

    // ── The Stop button ──────────────────────────────────────────────────────
    //
    // Every case here goes through `stop_fitness_run` or `fitness_runs` first,
    // which is not an accident: the persisted-stop cache is process state with
    // a two-second TTL shared by every test in this module, and both entry
    // points reset it before reading — so a cache warmed by whichever test ran
    // last cannot decide what this one sees.

    /// An hour before [`NOW`] — past the 45-minute staleness bound.
    const HOUR_AGO: &str = "2025-12-31T11:00:00.000Z";
    const NOW: &str = "2026-01-01T12:00:00.000Z";

    fn stop_deps(s: &Store) -> SurfaceDeps {
        let mut d = panic_deps();
        d.read_setting = store_read(s.clone());
        d.write_setting = store_write(s.clone());
        d.eval_sweep_statuses = Arc::new(|_| Box::pin(async { Ok(HashMap::new()) }));
        d.stop_eval_sweep = Arc::new(|_| false);
        d.models = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        d.now_iso = Arc::new(|| NOW.to_string());
        d
    }

    fn running_row(model: &str) -> Value {
        row("running", model, "now", None)
    }

    fn stale_row(model: &str) -> Value {
        row("running", model, HOUR_AGO, Some(HOUR_AGO))
    }

    fn fresh_row(model: &str) -> Value {
        row("running", model, HOUR_AGO, Some(NOW))
    }

    fn stop_list(s: &Store) -> Vec<String> {
        get(s, STOP_KEY)
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn stop_reaches_a_run_this_instance_does_not_hold_which_is_the_bug() {
        let _sole = sole_runs().await;
        // The request used to be a boolean on an in-process map, so it could
        // only reach a run whose closure lived in the module instance the
        // request happened to hit. One HMR reload — or one restart, or a
        // second process — and Stop returned `stopped: false` while the sweep
        // carried on for another twenty minutes. Observed exactly that, three
        // sweeps at once.
        let s = store();
        put(&s, RUNS_KEY, json!({ "spark/a": running_row("spark/a") }));
        let d = stop_deps(&s);
        let out = stop_fitness_run(Some("spark/a"), &d).await;
        assert!(out.stopped);
        assert_eq!(stop_list(&s), vec!["spark/a"]);
        // And the sweep, wherever it is running, can see it.
        assert!(stop_requested_for("spark/a", &d).await.unwrap());
    }

    #[tokio::test]
    async fn stop_stops_every_live_run_when_asked_for_all_of_them() {
        let _sole = sole_runs().await;
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/a": running_row("spark/a"), "spark/b": running_row("spark/b") }),
        );
        let out = stop_fitness_run(None, &stop_deps(&s)).await;
        assert!(out.stopped);
        let mut asked = stop_list(&s);
        asked.sort();
        assert_eq!(asked, vec!["spark/a", "spark/b"]);
    }

    #[tokio::test]
    async fn stop_reports_honestly_when_there_is_nothing_running_to_stop() {
        let _sole = sole_runs().await;
        let s = store();
        let out = stop_fitness_run(None, &stop_deps(&s)).await;
        assert!(!out.stopped);
    }

    // ── Orphans ──────────────────────────────────────────────────────────────
    //
    // A run is a task inside a process; its status is a row in the database. A
    // restart separates the two, and the row went on claiming `running` for
    // ever: the panel counted it against the concurrency limit, `full` went
    // true, and Stop wrote a request that no living thing would ever read. Two
    // of them accumulated in one afternoon of restarts, which is how this was
    // found.

    #[tokio::test]
    async fn reports_a_run_that_stopped_breathing_as_failed_not_as_running() {
        let _sole = sole_runs().await;
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/dead": stale_row("spark/dead") }),
        );
        let out = fitness_runs(&stop_deps(&s)).await.unwrap();
        let dead = out
            .runs
            .iter()
            .find(|r| r.get("model").and_then(Value::as_str) == Some("spark/dead"))
            .unwrap();
        assert_eq!(dead.get("state").and_then(Value::as_str), Some("error"));
        assert!(
            dead.get("error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .contains("interrupted")
        );
        // And it stops holding a concurrency slot, which is what made the
        // panel refuse to start anything after two restarts.
        assert!(!out.full);
    }

    #[tokio::test]
    async fn leaves_a_run_that_is_still_breathing_alone() {
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/live": fresh_row("spark/live") }),
        );
        let out = fitness_runs(&stop_deps(&s)).await.unwrap();
        let live = out
            .runs
            .iter()
            .find(|r| r.get("model").and_then(Value::as_str) == Some("spark/live"))
            .unwrap();
        assert_eq!(live.get("state").and_then(Value::as_str), Some("running"));
    }

    #[tokio::test]
    async fn does_not_rewrite_the_row_on_a_read() {
        // A read is not the place to take a durable decision: two instances
        // reading at once would both write, and a merely slow run would have
        // its row destroyed by whoever looked first. Reporting is enough.
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/dead": stale_row("spark/dead") }),
        );
        let _ = fitness_runs(&stop_deps(&s)).await;
        let stored = get(&s, RUNS_KEY).unwrap();
        assert_eq!(stored["spark/dead"]["state"], json!("running"));
    }

    #[tokio::test]
    async fn stop_clears_an_orphan_rather_than_leaving_a_note_nobody_reads() {
        let _sole = sole_runs().await;
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/dead": stale_row("spark/dead") }),
        );
        let out = stop_fitness_run(Some("spark/dead"), &stop_deps(&s)).await;
        assert!(out.stopped);
        let stored = get(&s, RUNS_KEY).unwrap();
        assert_eq!(stored["spark/dead"]["state"], json!("error"));
        assert_eq!(stored["spark/dead"]["finishedAt"], json!(NOW));
        // The note is pointless now, and left behind it would stop the NEXT run
        // on this model after one case — a bug this file has already had once.
        assert_eq!(stop_list(&s), Vec::<String>::new());
    }

    #[tokio::test]
    async fn stop_all_clears_an_orphan_which_is_how_the_panel_button_calls_it() {
        let _sole = sole_runs().await;
        // THE CASE THE NAMED-MODEL TEST ABOVE CANNOT REACH, and it was broken
        // for exactly one commit. Stop-all builds its target list by asking
        // which runs are live — and once the read started reporting a stale row
        // as `error` to keep it off the panel, that list no longer contained
        // the orphan. Stop returned `stopped: false` while the row went on
        // saying `running` for ever, which is the original bug wearing the
        // fix's clothes.
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/dead": stale_row("spark/dead") }),
        );
        let out = stop_fitness_run(None, &stop_deps(&s)).await;
        assert!(out.stopped);
        let stored = get(&s, RUNS_KEY).unwrap();
        assert_eq!(stored["spark/dead"]["state"], json!("error"));
        assert_eq!(stop_list(&s), Vec::<String>::new());
    }

    #[tokio::test]
    async fn stop_does_not_clear_a_live_run_belonging_to_another_instance() {
        let _sole = sole_runs().await;
        // Killing that row from here would report a sweep as failed while it
        // was still spending money. The heartbeat is what tells the two apart.
        let s = store();
        put(
            &s,
            RUNS_KEY,
            json!({ "spark/live": fresh_row("spark/live") }),
        );
        let d = stop_deps(&s);
        stop_fitness_run(Some("spark/live"), &d).await;
        let stored = get(&s, RUNS_KEY).unwrap();
        assert_eq!(stored["spark/live"]["state"], json!("running"));
        // It gets the ordinary request instead — the run reads it between
        // cases.
        assert_eq!(stop_list(&s), vec!["spark/live"]);
    }

    // ── The live log ─────────────────────────────────────────────────────────

    #[test]
    fn the_live_log_classifies_each_case_into_the_vocabulary_the_terminal_colours_by() {
        let mut checked = case("checked");
        checked.task = TaskVerdict::Fail;
        checked.task_error = Some("wrong shape".into());
        let mut ours = case("ours");
        ours.gap = Some("the fixture never gave it the id".into());
        let mut never = case("never");
        never.skipped = Some("no tool loop on this candidate".into());
        let mut slow = case("slow");
        slow.timed_out = true;
        let mut broke = case("broke");
        broke.contract_held = false;
        broke.error = Some("gateway completion 429".into());
        let out = live_log(&[case("ok"), checked, ours, never, slow, broke]);
        let verdicts: Vec<&str> = out.iter().map(|l| l.verdict.as_str()).collect();
        assert_eq!(
            verdicts,
            vec!["pass", "fail", "gap", "skip", "timeout", "error"]
        );
    }

    #[test]
    fn the_live_log_puts_a_skip_and_a_timeout_ahead_of_the_contract_flags_they_both_carry() {
        // Both land with `contractHeld: false` — the same zero every
        // unmeasured field carries. Reading either as an error is the exact
        // mistake the separate verdicts exist to prevent.
        let mut skip = case("skip");
        skip.skipped = Some("no tool loop".into());
        skip.contract_held = false;
        assert_eq!(live_log(&[skip])[0].verdict, LogVerdict::Skip);
        let mut timed_out = case("slow");
        timed_out.timed_out = true;
        timed_out.contract_held = false;
        assert_eq!(live_log(&[timed_out])[0].verdict, LogVerdict::Timeout);
    }

    #[test]
    fn the_live_log_carries_the_reason_error_first() {
        // When a case both errored and failed its check, the error is the
        // cause of the failure beside it.
        let mut c = case("broke");
        c.contract_held = false;
        c.error = Some("gateway completion 429: rate limited".into());
        c.task = TaskVerdict::Fail;
        c.task_error = Some("no value to grade".into());
        let lines = live_log(&[c]);
        assert!(lines[0].note.as_deref().unwrap_or("").contains("429"));
    }

    #[test]
    fn the_live_log_is_bounded_and_keeps_the_newest_lines() {
        // A log reads downward.
        let many: Vec<EvalCaseScore> = (0..LIVE_LOG_CAP + 25)
            .map(|i| case(&format!("c{i}")))
            .collect();
        let out = live_log(&many);
        assert_eq!(out.len(), LIVE_LOG_CAP);
        assert_eq!(out[out.len() - 1].case, format!("c{}", LIVE_LOG_CAP + 24));
    }

    #[test]
    fn the_live_log_stays_small_enough_to_poll_every_three_seconds() {
        // The whole point of a separate feed: `live_cases` cannot ship 250
        // full transcripts, and a failures-only list cannot show a sweep
        // moving. A line is roughly ninety bytes, so an entire sweep is tens
        // of kilobytes.
        let sweep_cases: Vec<EvalCaseScore> = (0..250)
            .map(|i| {
                let mut c = case(&format!("fixture number {i}"));
                c.harness = "work-session".into();
                c.latency_ms = 900 + i;
                c
            })
            .collect();
        assert!(
            serde_json::to_string(&live_log(&sweep_cases))
                .unwrap()
                .len()
                < 60_000
        );
    }

    // ── Clearing results ─────────────────────────────────────────────────────

    struct ClearWorld {
        s: Store,
        status_cleared: Arc<Mutex<Vec<String>>>,
        transcripts_cleared: Arc<Mutex<Vec<Option<String>>>>,
    }

    fn clear_world(ids: &[&str]) -> ClearWorld {
        let s = store();
        let mut index = FitnessIndex::new();
        for id in ids {
            index.insert(id.to_string(), entry(id, "2026-08-01T00:00:00.000Z"));
            put(&s, &record_key(id), json!({ "model": id }));
        }
        put(&s, INDEX_KEY, serde_json::to_value(&index).unwrap());
        ClearWorld {
            s,
            status_cleared: Arc::new(Mutex::new(Vec::new())),
            transcripts_cleared: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn clear_deps(w: &ClearWorld) -> SurfaceDeps {
        let mut d = panic_deps();
        d.read_setting = store_read(w.s.clone());
        d.write_setting = store_write(w.s.clone());
        d.models = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));
        let status = w.status_cleared.clone();
        d.clear_eval_status = Arc::new(move |m| {
            let status = status.clone();
            Box::pin(async move {
                status.lock().unwrap().push(m);
                Ok(())
            })
        });
        let transcripts = w.transcripts_cleared.clone();
        d.clear_transcripts = Arc::new(move |m| {
            let transcripts = transcripts.clone();
            Box::pin(async move {
                transcripts.lock().unwrap().push(m);
                Ok(7)
            })
        });
        d
    }

    #[tokio::test]
    async fn clear_drops_the_report_the_matrix_entry_the_resume_ledger_and_the_transcripts() {
        let w = clear_world(&["a/model", "b/model"]);
        let out = clear_fitness_results(Some("a/model"), &clear_deps(&w))
            .await
            .unwrap();
        assert_eq!(out.models, vec!["a/model"]);
        assert_eq!(out.reports, 1);
        assert_eq!(out.transcripts, 7);
        assert!(matches!(
            get(&w.s, &record_key("a/model")),
            Some(Value::Null)
        ));
        // THE HALF EVERYONE FORGETS. Leaving the resume ledger behind makes the
        // model read as untested and then resume into a run that is already
        // finished — a Start that returns instantly having bought nothing.
        assert_eq!(*w.status_cleared.lock().unwrap(), vec!["a/model"]);
        assert_eq!(
            *w.transcripts_cleared.lock().unwrap(),
            vec![Some("a/model".to_string())]
        );
        // The other model is untouched.
        let index = get(&w.s, INDEX_KEY).unwrap();
        let keys: Vec<&str> = index
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["b/model"]);
        assert_eq!(
            get(&w.s, &record_key("b/model")),
            Some(json!({ "model": "b/model" }))
        );
    }

    #[tokio::test]
    async fn clear_takes_every_tested_candidate_when_given_no_model() {
        let w = clear_world(&["a/model", "b/model"]);
        let out = clear_fitness_results(None, &clear_deps(&w)).await.unwrap();
        let mut models = out.models.clone();
        models.sort();
        assert_eq!(models, vec!["a/model", "b/model"]);
        assert_eq!(out.reports, 2);
        let index = get(&w.s, INDEX_KEY).unwrap();
        assert_eq!(index.as_object().unwrap().len(), 0);
        let mut cleared = w.status_cleared.lock().unwrap().clone();
        cleared.sort();
        assert_eq!(cleared, vec!["a/model", "b/model"]);
        // One sweep of the table, not one per model.
        assert_eq!(*w.transcripts_cleared.lock().unwrap(), vec![None]);
    }

    #[tokio::test]
    async fn clear_is_not_forget_it_never_touches_a_measured_capability() {
        // The distinction the two dialogs exist to keep: Forget drops what a
        // model CAN DO (nine probe calls somebody paid for); this drops what a
        // RUN FOUND. `SurfaceDeps.forget` is the capability eraser and must not
        // be reachable from here at all.
        let w = clear_world(&["a/model"]);
        let forgot: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut d = clear_deps(&w);
        let f = forgot.clone();
        d.forget = Arc::new(move |k| {
            let f = f.clone();
            Box::pin(async move {
                f.lock().unwrap().push(k);
                Ok(())
            })
        });
        clear_fitness_results(Some("a/model"), &d).await.unwrap();
        assert!(forgot.lock().unwrap().is_empty());
    }

    // ── Speed ────────────────────────────────────────────────────────────────

    #[test]
    fn speed_is_the_median_and_p95_of_the_cases_that_were_actually_measured() {
        let cases: Vec<EvalCaseScore> = [1000i64, 2000, 3000, 4000, 90_000]
            .iter()
            .enumerate()
            .map(|(i, latency)| {
                let mut c = case(&format!("c{i}"));
                c.latency_ms = *latency;
                c.wall_ms = *latency;
                c
            })
            .collect();
        let s = speed_of(&cases, 1).unwrap();
        assert_eq!(s.p50, 3000);
        assert_eq!(s.p95, 90_000);
        assert_eq!(s.concurrency, 1);
        // A median over five is a different claim from a median over two
        // hundred, and a supplemental pass makes small samples ordinary.
        assert_eq!(s.sample, 5);
    }

    #[test]
    fn speed_ignores_cases_that_never_called_the_model() {
        // A skipped case and one the provider never answered have no latency
        // to speak of. Averaging their zeros in would make a badly-served
        // deployment read as a fast model, which is the exact inversion this
        // page exists to prevent.
        let mut a = case("a");
        a.latency_ms = 4000;
        a.wall_ms = 4000;
        let mut b = case("b");
        b.latency_ms = 0;
        b.skipped = Some("no tool loop on this candidate".into());
        let mut c = case("c");
        c.latency_ms = 0;
        c.skipped = Some("rate limits on every attempt".into());
        let s = speed_of(&[a, b, c], 1).unwrap();
        assert_eq!(s.p50, 4000);
    }

    #[test]
    fn speed_measures_elapsed_as_a_timeline_so_concurrency_does_not_inflate_it() {
        // Four cases of 10s each, all started together, is ten seconds of wall
        // clock — not forty. A sum would report the sweep as four times longer
        // than an admin waited, and the whole reason to run wide is that it is
        // not.
        let cases: Vec<EvalCaseScore> = (0..4)
            .map(|i| {
                let mut c = case(&format!("c{i}"));
                c.latency_ms = 10_000;
                c.wall_ms = 10_000;
                c.started_at = "2023-11-14T22:13:20.000Z".into();
                c
            })
            .collect();
        let s = speed_of(&cases, 4).unwrap();
        assert_eq!(s.elapsed_ms, 10_000);
        assert!((s.per_minute - 24.0).abs() < 1e-9);
        assert_eq!(s.concurrency, 4);
    }

    #[test]
    fn speed_is_null_when_nothing_was_measured_rather_than_zero() {
        // Zero would draw a Speed column reading "0ms" for a model nothing ran
        // on, which is the fastest cell on the page and a lie.
        assert!(speed_of(&[], 1).is_none());
        let mut c = case("never");
        c.skipped = Some("never ran".into());
        c.latency_ms = 0;
        assert!(speed_of(&[c], 1).is_none());
    }

    // ── Speed is a rate, not a duration ──────────────────────────────────────

    #[test]
    fn speed_reports_the_rate_the_model_generates_at() {
        // 300 tokens in 3s and 100 in 1s are the SAME model speed. A per-case
        // latency would call the first one three times slower, which is a fact
        // about which fixture ran rather than about the model.
        let mut long = case("long");
        long.latency_ms = 3000;
        long.completion_tokens = 300;
        long.wall_ms = 3000;
        let mut short = case("short");
        short.latency_ms = 1000;
        short.completion_tokens = 100;
        short.wall_ms = 1000;
        let s = speed_of(&[long, short], 1).unwrap();
        assert_eq!(s.tokens_per_second, Some(100.0));
        // The duration is still there — "how long do I wait for a fixture" is
        // also a real question, it is just not the one a column comparing
        // models answers. (Nearest-rank over two samples takes the lower.)
        assert_eq!(s.p50, 1000);
        assert_eq!(s.p95, 3000);
    }

    #[test]
    fn speed_takes_the_median_per_case_rather_than_total_tokens_over_total_time() {
        // An aggregate would be inflated by concurrency (the wall clock
        // overlaps) and dominated by one long generation. Each case's own rate
        // is a fact about that request; the median of them is a fact about the
        // model.
        let cases: Vec<EvalCaseScore> = [(10i64), (20), (3000)]
            .iter()
            .enumerate()
            .map(|(i, tokens)| {
                let mut c = case(&format!("c{i}"));
                c.latency_ms = 1000;
                c.wall_ms = 1000;
                c.completion_tokens = *tokens;
                c
            })
            .collect();
        assert_eq!(speed_of(&cases, 1).unwrap().tokens_per_second, Some(20.0));
    }

    #[test]
    fn speed_rate_is_null_when_nothing_generated_enough_to_measure() {
        // A sweep of contract failures produces no completions. Zero would
        // read as the slowest model on the page rather than as an absent
        // measurement.
        let mut c = case("none");
        c.latency_ms = 500;
        c.wall_ms = 500;
        c.completion_tokens = 0;
        let s = speed_of(&[c], 1).unwrap();
        assert_eq!(s.tokens_per_second, None);
        assert_eq!(s.p50, 500);
    }
}

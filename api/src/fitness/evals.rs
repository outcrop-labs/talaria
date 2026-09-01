// TIER 2 OF THE MODEL FITNESS SUITE — harness conformance. Port of
// ui/src/server/fitness/evals.ts.
//
// WHAT IT ANSWERS, and it is not "is this a good model": for each harness in
// THIS install, can the candidate model hold that harness's contract, and where
// exactly does it break? A new model release should be a fifteen-minute sweep
// and a swap, not a week of production surprises.
//
// THIS IS A DRIVER, NOT A SUBSYSTEM. Everything it needs already crossed:
//   registry.rs    enumerates the harnesses, already type-erased
//   define.rs      `EvalCase` — a fixture input plus a deterministic `check`
//   run.rs         `run_harness`, which already takes `ctx.model` (the pin) and
//                  `ctx.deps` (the seam the runner's own tests drive it
//                  through). Replaying a fixture against a candidate is those
//                  two fields and nothing else.
// So the whole of tier 2 is: for every fixture, pin the candidate, run it, and
// read the numbers off the row the runner already writes.
//
// THE FIVE NUMBERS, and why `repairRate` is the one that matters:
//   contractRate  the contract held on the FIRST attempt
//   repairRate    the contract held at all, after the repair turn (CUMULATIVE —
//                 see the field comment). A model at 40% first-pass and 95%
//                 after one repair is USABLE; one at 40/45 is not, and until
//                 this file existed nothing in Talaria could tell those two
//                 apart. That distinction is the entire argument for audit 1.4.
//   taskScore     the fixture's own deterministic `check`. No model judging a
//                 model.
//   guardRate     guard findings per run, from the pass the runner already does
//   latency/cost  what the sweep actually spent
//
// THE PREDICATE IS THE RUNNER'S OWN, READ OFF THE ROW IT WRITES. This file
// NEVER decides "did the contract hold": it captures the `HarnessRunRow` the
// runner hands to `record_run` — the literal row production reads — and scores
// `row.schema_valid`, `row.repairs`, `row.findings` and `row.latency_ms`. If
// the benched number and the observed number ever diverge, it is because the
// model changed, not because the ruler did. The fixture's `check` is scored
// SEPARATELY, as `taskScore`, and the two are allowed to disagree — that is
// what `optimistic` counts.
//
// WHAT THE SWEEP DELIBERATELY DOES NOT WRITE: `harness_runs` rows and
// `guard_findings` rows. Both tables are the OBSERVED half of the fitness page,
// shown next to the benched half, and a sweep that filed into them would move
// the number it is being compared against. The guard pass still RUNS (that is
// where `guardRate` comes from); only the filing is suppressed. Token spend is
// real spend and still reaches `usage_events` through the transports, which is
// why the sweep's caller names itself.
//
// THE CLOCK RACES BY DROPPING, not by aborting. The TS forwards an
// AbortSignal; the Rust runner has no signal slot, so the case's future is
// raced against the clock with `tokio::select!` and the loser is DROPPED —
// which releases the socket just as surely, and deterministically: a timed-out
// case's run row never lands here, where in TS it landed or did not depending
// on how fast the abort rejection ran. A timed-out case is excluded from every
// rate anyway, so nothing scored depends on the difference.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::capability_reach::{self, DbReach, ReachDeps, Supplier};
use crate::fitness::toolbox::credential_tools::CredentialSandbox;
use crate::fitness::toolbox::dry_run::{
    sandbox_transport, turn_budget, DispatchSandbox, DryRunResult,
};
use crate::fitness::toolbox::hermes_tools::WorkbenchSandbox;
use crate::fitness::toolbox::sandbox::{DispatchResult, Sandbox, SandboxCall, SandboxOptions};
use crate::gateway::guard::{self, GuardMode};
use crate::gateway::settings::{get_setting, set_setting};
use crate::gateway::usage::estimate_tokens;
use crate::harness::define::{
    is_gap, CheckCtx, CheckResult, EvalBand, EvalCase, HarnessDefinition, Output,
};
use crate::harness::defs::research::{
    tool_search_transport, SearchSink, ToolSearchDeps,
};
use crate::harness::registry::{builtin_activity_harnesses, RegisteredHarness};
use crate::harness::run::{
    real_deps as runner_real_deps, run_harness, BoxFut, HarnessDeps, HarnessRunRow,
    RecordFindingsFn, RecordRunFn, TransportFn,
};
use crate::harness::transport::{
    offers_tool_definitions, runs_own_tool_loop, ToolPolicy, TransportRequest,
};
use crate::state::AppState;

// ── The scoring surface ──────────────────────────────────────────────────────

/// The fixture's own verdict. `Unscored` is not a middle grade — it is "there
/// was no model value to grade", for a contract failure or a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskVerdict {
    Pass,
    Fail,
    Unscored,
}

/// One fixture, replayed once against the candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCaseScore {
    pub harness: String,
    /// `EvalCase.name`. Unique within a harness; `case_key` joins the two.
    pub case: String,
    /// `EvalCase.band`, defaulted. Carried on the CASE rather than looked up
    /// from the registry later, because a report read back from the archive
    /// has to keep meaning what it meant when it was written — a fixture
    /// re-banded next quarter must not silently re-band last quarter's run.
    pub band: EvalBand,

    /// THE SWEEP NEVER CALLED THE MODEL, and this is not a result about it.
    ///
    /// Non-null carries the sentence saying why, written for the admin reading
    /// the drill-down. Every field below it is a zero that means "not
    /// measured", and `score_harness` excludes the case from every rate rather
    /// than averaging the zeros in — which is the whole reason the field
    /// exists. It covers a harness this candidate's transport cannot drive,
    /// the capability floor declining to ask, a rate limit that never lifted,
    /// and a deployment that cannot reach the model at all.
    pub skipped: Option<String>,

    /// THE CONTRACT HELD — `harness_runs.schema_valid`, taken from the row the
    /// runner wrote rather than recomputed here. See the file header.
    pub contract_held: bool,
    /// It held WITHOUT a repair turn. `repairs == 0 && contract_held`, which is
    /// exact: the runner's attempt loop breaks the moment the contract holds,
    /// so a zero repair count on a valid run means the first reply was valid.
    pub first_pass: bool,
    pub repairs: i64,
    /// The model produced a reply the contract could be applied to. False for
    /// every way a run ends without one, and in each of those `error` carries
    /// the runner's own sentence naming which.
    pub answered: bool,

    /// THE FIXTURE'S OWN DETERMINISTIC CHECK. `Unscored` when the contract
    /// failed (no model value to grade — counting it would double-charge one
    /// fault) and for a harness whose `on_failure` hands back a declared
    /// fallback value, which would award the model task points for a constant
    /// its author wrote.
    pub task: TaskVerdict,
    /// The fixture's one-line reason, verbatim. This is what an admin reads in
    /// the drill-down, which is why a fixture's check is written for a human.
    pub task_error: Option<String>,
    /// OUR GAP, not the model's failure. Non-null means this fixture could not
    /// fairly ask its question, so the case is unscored and this sentence is
    /// reported to the people who own the harness. See `CheckResult`.
    pub gap: Option<String>,

    /// Guard findings this run is EVIDENCE for — `harness_runs.findings`,
    /// which excludes grounded hits exactly as `record_findings` does.
    pub findings: i64,
    /// THE RUNNER'S OWN MEASURE of the final attempt — `harness_runs
    /// .latency_ms`, covering render, the model turns, the repair round-trip
    /// and the guard pass. It is what the observed-vs-tested comparison is
    /// computed from, so it must stay the number production records.
    pub latency_ms: i64,
    /// WHEN THE SWEEP STARTED THIS CASE, ISO. Needed to reconstruct a
    /// timeline: under concurrency, `latency_ms` alone cannot tell a slow model
    /// from four fast cases queued behind each other.
    pub started_at: String,
    /// WHAT THE CASE COST THE SWEEP, wall clock, INCLUDING everything
    /// `latency_ms` excludes: sandbox construction, the closing turn of a tool
    /// loop, and — the big one — every retry of a rate-limited or lost request.
    /// A case whose first two attempts vanished and whose third took 4s has
    /// `latencyMs: 4000` and `wallMs: 124000`, and only the second number
    /// explains where the sweep's afternoon went.
    pub wall_ms: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// Null when nothing priced the tokens — see `EvalDeps.price`.
    pub cost_usd: Option<f64>,
    /// True when the token counts are a chars/4 estimate because the transport
    /// reported no usage. A cost built on estimated tokens is an estimate.
    pub estimated: bool,

    /// The case did not settle inside the bound and the sweep moved on. A
    /// hanging harness must never strand the sweep.
    pub timed_out: bool,

    /// THE CONTRACT HELD AND THE FIXTURE REJECTED THE VALUE ANYWAY. Not
    /// automatically a bug: expected where the fixture grades QUALITY the
    /// contract deliberately does not police, a bug where it asserts something
    /// the CALLER depends on — the tell is `HarnessScore.verifies`.
    pub optimistic: bool,

    /// The runner's failure sentence, redacted and bounded by the runner. Null
    /// on a clean run.
    pub error: Option<String>,
    /// DRILL-DOWN, kept only for cases that failed something — the actual
    /// prompt and the actual response, which is what makes a red cell
    /// trustworthy instead of merely alarming. A clean case carries neither:
    /// seventy passing transcripts in a settings row is an archive, not
    /// telemetry.
    pub prompt: Option<String>,
    pub raw: Option<String>,

    /// THE WHOLE CONVERSATION, for a case that had one. Null for a case that
    /// took one turn (there is nothing here prompt/raw do not already say) and
    /// for a clean one.
    pub turns: Option<Vec<EvalTurn>>,

    /// EVERY UPSTREAM CALL THIS CASE MADE, kept whenever the case did not
    /// finish cleanly. The evidence behind a timeout.
    pub upstream: Option<Vec<UpstreamAttempt>>,

    /// WHAT THE MODEL ACTUALLY DID — the sandbox's call log, verbatim and in
    /// order. Kept for EVERY dry-run case including the passing ones, unlike
    /// the transcript, because it is small and it is the primary artifact
    /// every behavioural fixture asserts over. An admin comparing two models
    /// on one fixture is comparing these lists.
    pub calls: Option<Vec<EvalToolCall>>,
}

/// One turn of a recorded conversation. Mirrors the runner's `Message` rather
/// than reusing it: this shape is PERSISTED into a settings row and read back
/// by a UI months later, so it is flat, bounded, and free of anything the
/// runner might redefine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalTurn {
    pub role: String,
    pub content: String,
    /// Names only. The arguments are on `calls`, where they are not duplicated
    /// once per turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<String>>,
}

/// One tool call as the sandbox saw it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalToolCall {
    pub tool: String,
    /// JSON, bounded. Rendered rather than parsed by the UI.
    pub args: String,
    /// What came back, bounded — the model saw this, so an admin should too.
    pub result: Option<String>,
    /// The tool refused. A refusal is a real event and reads very differently
    /// from a call that never happened.
    pub error: Option<String>,
}

/// One model call inside one case, as the sweep saw it.
///
/// WHY THIS EXISTS. A timed-out case used to report `the case did not finish
/// inside 60000ms` and nothing else, which is the least useful true sentence
/// available: it cannot distinguish a model that is genuinely slow from a
/// request that never came back, from a case that spent its budget on four
/// retries, from one that never reached the provider at all. These are cheap —
/// three fields per model call — and they turn the next report into a
/// diagnosis instead of a symptom.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamAttempt {
    /// Wall time for this call. For one still in flight when the case was
    /// killed, how long it had been waiting.
    pub ms: i64,
    /// Did it come back at all? False for both an error and a call still open.
    pub settled: bool,
    /// The transport's own sentence when it threw. Null on a clean reply.
    pub error: Option<String>,
}

/// Everything the sweep needs to know about a harness that is not a score.
/// Split out so `score_harnesses` stays pure over recorded cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessMeta {
    pub id: String,
    pub label: String,
    /// The TS `HarnessSource` vocabulary, as the string the admin panel reads.
    pub source: String,
    pub output_kind: String,
    /// The model's-own-tool-loop policy, spelled "none"/"own". Read by
    /// `harness_skip_reason`, which needs it before the harness runs.
    pub tools: String,
    pub requires: Vec<String>,
    /// Does this harness declare the input-relational half of its contract?
    /// See `EvalCaseScore.optimistic`.
    pub verifies: bool,
    /// CAN a repair turn happen here at all? The runner allows repairs only on
    /// JSON output, and a text harness cannot have one — without this flag
    /// `repairYield` would print 0 ("the repair turn rescued nothing") where
    /// the honest answer is "no repair turn was ever sent".
    pub repairable: bool,
}

/// The per-harness column of the fitness matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessScore {
    #[serde(flatten)]
    pub meta: HarnessMeta,
    /// Fixtures this sweep RAN — skipped ones are not cases, they are
    /// absences. Every rate below is over this denominator, so a harness the
    /// candidate cannot be tested on reports zero of everything with `cases:
    /// 0`, which every consumer already reads as "no evidence". A denominator
    /// that counted the skips would print 0% instead, and 0% is a verdict.
    pub cases: i64,
    /// Fixtures the sweep declined to run against this candidate, with the
    /// reason on `skip_reason`. Reported so a full-green matrix can still say
    /// what it did not look at.
    pub skipped: i64,
    /// FIXTURES THAT COULD NOT FAIRLY ASK THEIR QUESTION — our gap, not the
    /// model's failure. `gap_reasons` is what the run reports back to whoever
    /// owns the harness.
    pub gaps: i64,
    pub gap_reasons: Vec<String>,
    /// Why, verbatim from the first skipped case. Null when nothing was
    /// skipped.
    pub skip_reason: Option<String>,
    /// Cases that reached a verdict — everything except a timeout.
    pub scored: i64,

    /// THE CONTRACT HELD ON THE FIRST ATTEMPT, over all cases.
    pub contract_rate: f64,
    /// THE CONTRACT HELD AT ALL, over all cases — CUMULATIVE, so it is always
    /// >= `contract_rate` and the pair reads the way the audit states it:
    /// 40/95 is a usable model with a repair path, 40/45 is not.
    pub repair_rate: f64,
    /// Of the cases that failed first, the share the repair turn RECOVERED.
    /// The conditional number, kept because it is the one that says whether
    /// spending a second round-trip on this model is worth it.
    ///
    /// Null when the question does not arise: nothing failed first, or the
    /// harness cannot repair at all. Zero means the repair turn ran and
    /// rescued nothing, which is a different and much worse fact.
    pub repair_yield: Option<f64>,

    /// Fixture checks passed, over the cases that were task-scorable. Null
    /// when none were — a model that never held the contract has no task
    /// score, and printing 0 would blame it twice.
    pub task_score: Option<f64>,
    /// THE SAME NUMBER, SPLIT BY DIFFICULTY, in registry band order. Null per
    /// band when that band had nothing scorable.
    ///
    /// WHY IT IS WORTH THE FIELD: one flat rate cannot tell "competent, loses
    /// the hard edge cases" from "unreliable on the basics", and those are
    /// different purchasing decisions. A 70% that is easy 100 / standard 100 /
    /// hard 20 is a fine Utility model; a 70% that is easy 70 / standard 70 /
    /// hard 70 is a model that fails one job in three at random, which is
    /// worse in every way that matters.
    pub band_scores: BandScores,
    /// Findings per run.
    pub guard_rate: f64,
    pub answered_rate: f64,

    pub latency_p50: i64,
    pub latency_p95: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// Sum of the priced cases; null when nothing was priced.
    pub cost_usd: Option<f64>,
    pub estimated: bool,

    pub timeouts: i64,
    pub optimistic: i64,
}

/// The three band columns, in the registry's fixed order — the JSON shape the
/// admin panel reads (`{ easy, standard, hard }`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandScores {
    pub easy: Option<f64>,
    pub standard: Option<f64>,
    pub hard: Option<f64>,
}

/// The sweep's lifecycle, as the status row spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvalSweepState {
    Idle,
    Running,
    Stopped,
    Done,
    Error,
}

/// THE RESUMABLE, BOUNDED RUN, in `app_settings` — the same shape and the same
/// lifecycle as the reindex status, deliberately. Talaria has one long-run
/// mechanism and this is it; a second one would be a second set of stuck-state
/// bugs to learn about.
///
/// It carries the scored cases as well as the progress, and that is what makes
/// the run RESUMABLE rather than merely observable: a sweep stopped by an admin
/// (or interrupted by a deploy) can be restarted and will skip what it already
/// paid for. Seventy cases of bounded text is a few KB.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalSweepStatus {
    pub state: EvalSweepState,
    /// The candidate this sweep is about. Resuming only ever continues a sweep
    /// of the SAME model — scores from two models in one set would be a matrix
    /// cell that means nothing.
    pub model: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Cases finished / cases planned.
    pub done: i64,
    pub total: i64,
    /// The harness currently running, for the progress line.
    pub harness: Option<String>,
    pub error: Option<String>,
    pub cases: Vec<EvalCaseScore>,
}

/// The finished (or stopped) sweep, scored.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalSweep {
    pub model: String,
    pub state: EvalSweepState,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub done: i64,
    pub total: i64,
    pub error: Option<String>,
    pub harnesses: Vec<HarnessScore>,
    pub cases: Vec<EvalCaseScore>,
    /// Registered harnesses that declare NO fixtures. They are invisible to
    /// tier 2 — not passing, not failing — and an admin reading a full green
    /// matrix deserves to know which columns were never tested.
    pub unfixtured: Vec<String>,
    /// Was the guard pass actually on? With `mode: 'off'` every `guardRate` is
    /// zero, and zero-because-off must not read as zero-because-clean.
    pub guarded: bool,
    /// HOW WIDE THE SWEEP RAN. Reported because it changes what `latencyP50`
    /// MEANS: at four wide the number includes queueing at the provider, so it
    /// is "what a call costs under this load" rather than "what a call costs".
    pub concurrency: SweepConcurrency,
    /// THE CASES THIS PASS ACTUALLY RAN, as opposed to the ones it inherited
    /// from an earlier one. Speed is measured over these and never over
    /// `cases` — a supplemental pass that ran seven fixtures must not report a
    /// latency computed from two hundred inherited ones measured last week at
    /// a different width.
    pub measured: Vec<EvalCaseScore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepConcurrency {
    /// What the run asked for.
    pub requested: usize,
    /// What it ended at. Below `requested` means the valve was still closed
    /// when the sweep finished; back AT `requested` means it closed and
    /// reopened, which is the ordinary shape of a run that hit one bad minute.
    pub ended: usize,
    /// THE NARROWEST IT EVER RAN, which is the number that explains the
    /// timings. `ended` alone cannot: a sweep that spent two hundred cases at
    /// width 1 and recovered on the last ten ends at 4 and looks like it never
    /// struggled.
    pub low: usize,
    /// THE PROVIDER PUSHING BACK, verbatim, whether or not there was width
    /// left to give up. Non-null is a fact about the DEPLOYMENT and must never
    /// be read as a fact about the model.
    pub narrowed_because: Option<String>,
}

// ── Injected edges ───────────────────────────────────────────────────────────

/// Every edge owns its inputs (each closure clones what it needs before
/// boxing), the same contract the runner's own dep set states.
pub type HarnessesFn = Arc<dyn Fn() -> BoxFut<Vec<RegisteredHarness>> + Send + Sync>;
/// THE TOOL THIS INSTALL SUPPLIES FOR A CAPABILITY, or None. Injected so the
/// sweep stays runnable with no database and no MCP registry anywhere near it.
/// Takes the capability by value — the boxed future it hands back is 'static,
/// so it cannot borrow the caller's string.
pub type SupplierFn = Arc<dyn Fn(String) -> BoxFut<Option<Supplier>> + Send + Sync>;
pub type ReadStatusFn = Arc<dyn Fn(String) -> BoxFut<EvalSweepStatus> + Send + Sync>;
pub type WriteStatusFn = Arc<dyn Fn(EvalSweepStatus) -> BoxFut<()> + Send + Sync>;
pub type ReadAllStatusFn =
    Arc<dyn Fn(Vec<String>) -> BoxFut<HashMap<String, EvalSweepStatus>> + Send + Sync>;
/// Dollars for one call's tokens, or None when this install cannot say.
///
/// DEFAULTS TO NONE ON PURPOSE. Talaria prices spend in exactly one place —
/// the priced view over `usage_events` — and the sweep's turns land in that
/// table through the real transports, so the fitness page can join the ledger
/// and get the same dollars everything else in the product quotes. Re-deriving
/// that formula here would be a second price that drifts from the first.
pub type PriceFn = Arc<dyn Fn(String, i64, i64) -> BoxFut<Option<f64>> + Send + Sync>;
/// CAN this candidate run a harness that wants the model's OWN tool loop?
/// Asked once per sweep, before any harness runs.
pub type OwnToolsFn = Arc<dyn Fn(String) -> BoxFut<bool> + Send + Sync>;
/// CAN this candidate be handed tool DEFINITIONS — the other half of the
/// question above. Together they decide whether a tool-loop harness runs as
/// production runs it, is dry-run against the sandbox, or is honestly skipped.
pub type ToolDefsFn = Arc<dyn Fn(String) -> BoxFut<bool> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
/// The sync stop predicate a case polls — `stopRequested.has(model) ||
/// externallyStopped` in the TS.
pub type StopPred = Arc<dyn Fn() -> bool + Send + Sync>;
pub type OnCaseFn = Arc<dyn Fn(EvalCaseScore) -> BoxFut<()> + Send + Sync>;

/// The sweep's world. `harness_deps` replaces the runner's dep set WHOLESALE
/// (the Rust runner's one spelling of the TS `Partial` override — see
/// `RunContext.deps`), and the sweep layers its own transport, `record_run`
/// capture and findings suppression on top of whichever set it was handed;
/// everything else — model resolution, capability facts, the guard config —
/// stays REAL, because the capability floor refusing a weak model IS a tier-2
/// result and a fake that never refuses would be testing the fake.
pub struct EvalDeps {
    pub harnesses: HarnessesFn,
    pub supplier: Option<SupplierFn>,
    /// HOW A SUPPLIED TOOL IS ACTUALLY CALLED. Defaults to the same dispatcher
    /// production uses. Injected so a test can decide what search RETURNS —
    /// without this seam the only way to exercise the empty-search gap was to
    /// let a unit test reach a live engine over the network and hope it found
    /// nothing, and a test whose verdict depends on the internet is not a test.
    pub search_tool: Option<crate::harness::defs::research::CallToolFn>,
    pub harness_deps: Option<Arc<HarnessDeps>>,
    /// THIS CANDIDATE'S checkpoint. Per model since sweeps run concurrently —
    /// a single row would have three candidates' cases overwrite each other.
    pub read_status: ReadStatusFn,
    pub write_status: WriteStatusFn,
    /// The named candidates' checkpoints, for the panel that draws the running
    /// sweeps. Takes the list rather than discovering it — the caller already
    /// knows which candidates have runs, and a prefix scan of `app_settings`
    /// to rediscover that would be a second, weaker source of the same list.
    pub read_all_status: ReadAllStatusFn,
    pub price: PriceFn,
    pub serves_own_tools: OwnToolsFn,
    pub accepts_tool_definitions: ToolDefsFn,
    pub now: NowFn,
}

/// THE CHECKPOINT — ONE ROW PER CANDIDATE, and the row is the reason a wide
/// sweep is affordable at all. A write is proportional to ONE candidate's own
/// cases and nothing else; ten concurrent sweeps write exactly what ten
/// sequential ones would, and no two of them touch the same row.
///
/// TWO OLDER SHAPES ARE STILL READ, never written — `harness_eval_runs` (the
/// shared map) and `harness_eval_status` (the single row before that) each
/// hold the resume point of whatever sweep was in flight when its shape
/// changed, and the loser of ignoring it is hours of paid-for cases.
fn run_key(model: &str) -> String {
    format!("harness_eval_run:{model}")
}
const RUNS_KEY: &str = "harness_eval_runs";
const LEGACY_STATUS_KEY: &str = "harness_eval_status";

/// What a candidate with no checkpoint reads as. Never an error.
pub fn idle_status() -> EvalSweepStatus {
    EvalSweepStatus {
        state: EvalSweepState::Idle,
        model: None,
        started_at: None,
        finished_at: None,
        done: 0,
        total: 0,
        harness: None,
        error: None,
        cases: Vec::new(),
    }
}

/// One candidate's checkpoint: its own row, then the two older shapes. The
/// fallbacks are read-only and exist so a sweep that was in flight when the
/// storage changed still resumes rather than re-buying its cases.
async fn read_run(pg: &sqlx::PgPool, model: &str) -> EvalSweepStatus {
    let parse = |v: Value| serde_json::from_value::<EvalSweepStatus>(v).ok();
    if let Some(own) = parse(get_setting(pg, &run_key(model), Value::Null).await) {
        return own;
    }
    let shared = get_setting(pg, RUNS_KEY, json!({})).await;
    if let Some(hit) = shared.get(model).and_then(|v| parse(v.clone())) {
        return hit;
    }
    match parse(get_setting(pg, LEGACY_STATUS_KEY, Value::Null).await) {
        Some(legacy) if legacy.model.as_deref() == Some(model) => legacy,
        _ => idle_status(),
    }
}

/// The real edges: settings-table checkpoint, the same capability supplier the
/// platform itself resolves through, the transport's own answers to the two
/// tool questions, and NO price.
pub fn real_deps(state: &AppState) -> EvalDeps {
    let pg = state.pg.clone();
    let read_pg = state.pg.clone();
    EvalDeps {
        harnesses: Arc::new(|| Box::pin(async { builtin_activity_harnesses().to_vec() })),
        supplier: Some(Arc::new(move |capability| {
            let pg = pg.clone();
            Box::pin(async move {
                // The same resolver capability-reach uses, so the sweep
                // supplements exactly what production would supply — never a
                // second answer to "what does this install have". Talaria's
                // own tools count: without the platform half the sweep would
                // ask an empty MCP registry and run `research-search` with no
                // search tool while SearXNG answered queries on the same box.
                let reach = DbReach { pg: &pg };
                let servers = reach.servers().await;
                let providers = reach.providers().await;
                let platform = reach.platform().await;
                capability_reach::supplier_for(&capability, &servers, &providers, &platform)
            })
        })),
        search_tool: None,
        harness_deps: None,
        read_status: Arc::new(move |model| {
            let pg = read_pg.clone();
            Box::pin(async move { read_run(&pg, &model).await })
        }),
        // ONE ROW, ONE WRITER, NO READ FIRST. Nothing else writes this key, so
        // there is nothing to merge and nothing to serialize against.
        write_status: {
            let pg = state.pg.clone();
            Arc::new(move |status: EvalSweepStatus| {
                let pg = pg.clone();
                Box::pin(async move {
                    if let Some(model) = &status.model {
                        let value = serde_json::to_value(&status).unwrap_or(Value::Null);
                        let _ = set_setting(&pg, &run_key(model), &value).await;
                    }
                })
            })
        },
        read_all_status: {
            let pg = state.pg.clone();
            Arc::new(move |models: Vec<String>| {
                let pg = pg.clone();
                Box::pin(async move {
                    let mut out = HashMap::new();
                    for m in models {
                        let status = read_run(&pg, &m).await;
                        out.insert(m, status);
                    }
                    out
                })
            })
        },
        price: Arc::new(|_, _, _| Box::pin(async { None })),
        // A DEAD LISTING MUST NOT SKIP EVERYTHING. If the fleet listing is
        // unreachable the honest fallback is "assume it can run", which spends
        // one refusal per fixture and records it, rather than silently marking
        // three harnesses untestable on a transient outage.
        serves_own_tools: {
            let st = state.clone();
            Arc::new(move |model| {
                let st = st.clone();
                Box::pin(async move { runs_own_tool_loop(&st, &model).await })
            })
        },
        accepts_tool_definitions: {
            let st = state.clone();
            Arc::new(move |model| {
                let st = st.clone();
                Box::pin(async move { offers_tool_definitions(&st, &model).await })
            })
        },
        now: Arc::new(now_ms),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `new Date(ms).toISOString()` — UTC, milliseconds, trailing Z, byte-identical
/// to the TS spelling so a status row written by either side reads the same.
fn iso(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into())
}

/// One candidate's live status, for a polling admin panel.
pub async fn eval_sweep_status(pg: &sqlx::PgPool, model: &str) -> EvalSweepStatus {
    read_run(pg, model).await
}

/// Several candidates', for the panel that draws all the running sweeps.
pub async fn eval_sweep_statuses(
    pg: &sqlx::PgPool,
    models: &[String],
) -> HashMap<String, EvalSweepStatus> {
    let mut out = HashMap::new();
    for m in models {
        let status = read_run(pg, m).await;
        out.insert(m.clone(), status);
    }
    out
}

/// THROW AWAY THE RESUME LEDGER for one candidate.
///
/// A sweep's persisted status is BOTH its progress bar and the list of cases
/// it already paid for, so clearing the archived report without clearing this
/// leaves a model that looks untested and then resumes into a run that is
/// already finished — a Start that returns instantly having bought nothing.
/// The two always go together, which is why the surface's clear owns both.
pub async fn clear_eval_status(pg: &sqlx::PgPool, model: &str) {
    let mut status = idle_status();
    status.model = Some(model.to_string());
    let value = serde_json::to_value(&status).unwrap_or(Value::Null);
    let _ = set_setting(pg, &run_key(model), &value).await;
}

// ── Options ──────────────────────────────────────────────────────────────────

/// What one sweep was asked to do, beyond "every fixture against this model".
#[derive(Default)]
pub struct EvalOptions {
    /// THE BOUND ON ONE CASE (per turn — see `turns_per_case`). A harness that
    /// hangs must cost the sweep one case, not the whole run: the case is
    /// recorded `timed_out` and the sweep moves on.
    pub case_timeout_ms: Option<u64>,
    /// ASKED TO STOP FROM ANYWHERE, not just from this process. The in-process
    /// set below only reaches a sweep whose closure lives in the instance the
    /// request hit; the surface supplies a reader over the persisted request.
    pub should_stop: Option<Arc<dyn Fn(String) -> BoxFut<bool> + Send + Sync>>,
    /// ARCHIVE EVERY CASE, PASSING INCLUDED. The settings-row report keeps a
    /// transcript only for cases that failed something — right for a
    /// drill-down, useless for verification, because "did our fixture accept
    /// something weak" can only be answered from a PASSING transcript.
    pub archive_case: Option<Arc<dyn Fn(String, String, EvalCaseScore) -> BoxFut<()> + Send + Sync>>,
    /// Called once when a run ends, whatever way it ended.
    pub archive_prune: Option<Arc<dyn Fn(String) -> BoxFut<()> + Send + Sync>>,
    /// Only these harness ids. Empty/omitted means every registered harness.
    pub only: Option<Vec<String>>,
    /// Ignore a resumable status and start clean.
    pub restart: bool,
    /// KEEP THE PASSES, RE-ASK EVERYTHING ELSE — the middle setting between
    /// resume and restart, and the one an admin actually wants after a bad
    /// run. See `worth_retrying` for what counts as "everything else".
    pub retry_failed: bool,
    /// RUN WHAT HAS NEVER BEEN RUN, and nothing else — the mode that matters
    /// once a suite is being actively developed. It also PRUNES: a recorded
    /// case whose fixture no longer exists is a verdict about an assertion
    /// nobody can read, and leaving it in the ledger means the matrix is
    /// scored partly on questions the suite has stopped asking.
    pub supplement: bool,
    /// HOW MANY OF A HARNESS'S FIXTURES RUN AT ONCE. Clamped to
    /// `MAX_CONCURRENCY`; 1 restores the old strictly-sequential sweep.
    pub concurrency: Option<usize>,
    /// The gaps between retries of a rate-limited case. Injected so a test can
    /// drive the retry path in milliseconds instead of half a minute.
    pub pressure_backoff_ms: Option<Vec<u64>>,
    /// Replaces the whole dep set, the runner's one spelling of the TS
    /// `Partial` override.
    pub deps: Option<Arc<EvalDeps>>,
}

/// HOW MANY CASES RUN AT ONCE, and what the old sequential rule was
/// protecting.
///
/// THIS SWEEP WAS SEQUENTIAL ON PURPOSE, for two reasons that are both still
/// true and neither of which required going one at a time:
///
///   LATENCY STOPS MEANING WHAT THE PAGE SAYS. `latencyP50` under N-way
///   concurrency includes queueing at the provider. The fix is not to refuse
///   concurrency; it is to SAY SO — `EvalSweep.concurrency` is recorded and
///   the fitness page labels the number with it.
///
///   A SELF-HOSTED 14B BEHIND ONE GPU rate-limits, and those 429s would score
///   as contract failures. The fix is the pressure valve below: the sweep
///   drops its width when it sees rate-limit pressure and says it did.
///
/// FOUR, because a 247-fixture sweep one-at-a-time is most of an hour and the
/// first duplicate an admin runs is the one they stop watching.
pub const DEFAULT_CONCURRENCY: usize = 4;
pub const MAX_CONCURRENCY: usize = 8;

/// A reply that means "you are asking too fast", not "the model is bad". Both
/// halves matter: a 429 is unambiguous, and the 5xx family covers the
/// overloaded gateways that answer 502/503 under the same pressure.
static PRESSURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(429|too many requests|rate.?limit|502|503|504|overloaded|capacity)\b")
        .expect("a static regex compiles")
});

/// HOW MANY TIMES A PRESSURED CASE IS RE-RUN before the sweep gives up on it.
///
/// A 429 IS NOT A RESULT ABOUT THE MODEL. It is the provider saying "slower",
/// and scoring it as a contract failure is the same category error as scoring
/// a 401 as a model that cannot hold JSON. Three attempts with a widening
/// gap, because rate limits clear on a timescale of seconds and the sweep has
/// already halved its own width by the second one.
pub const PRESSURE_RETRIES: usize = 3;
pub const PRESSURE_BACKOFF_MS: [u64; 3] = [2_000, 8_000, 20_000];

/// A LOST REQUEST GETS ONE MORE CHANCE, NOT THREE, and the asymmetry is about
/// cost rather than principle. A rate limit comes back in milliseconds, so
/// three retries are nearly free. A request that is never answered costs the
/// WHOLE CASE BUDGET to discover, so three retries would turn one lost request
/// into four minutes. One retry doubles the cost and gives a genuine second
/// chance; a request that vanishes twice is telling you about the deployment.
pub const TIMEOUT_RETRIES: usize = 1;

/// HOW MANY CLEAN CASES IN A ROW REOPEN THE VALVE BY ONE LANE.
///
/// A VALVE THAT ONLY CLOSES IS A RATCHET, and a ratchet is the wrong shape for
/// a signal that is usually transient — the original valve spent a 247-case
/// sweep at width 1 because of one vanished HTTP call in its first minute,
/// and the archived reading said so plainly (`requested: 4, ended: 1`) and
/// nobody read it.
///
/// FIVE, AND WHY IT IS NOT ONE. Reopening after a single success would
/// oscillate — widen, hit the same limit, halve, repeat — turning a quiet
/// ceiling into a sawtooth that pays a 429 every few cases. Five clean cases
/// at the current width is evidence the deployment is actually serving it,
/// and stepping up by ONE keeps a real ceiling found instead of rediscovered.
pub const RECOVER_AFTER: usize = 5;

/// THE DEPLOYMENT CANNOT REACH THIS MODEL AT ALL — which is not a fact about
/// the model, and must never be scored as one.
///
/// THE RUN THAT FOUND THIS. A sweep recorded 247 failures, every one of them
/// `gateway completion 404: No allowed providers are available for the
/// selected model`, in 58 milliseconds each — the org's own no-train policy
/// pinned `provider.only` to a US pool that did not serve the model, and the
/// matrix reported it as a model that fails every harness in Talaria.
static UNREACHABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)no allowed providers|no endpoints found|not a valid model|model_not_found|does not exist or you do not have access|\b40[13]\b|invalid api key|unauthorized")
        .expect("a static regex compiles")
});

/// How many consecutive unreachable cases before the sweep stops asking. A
/// structural refusal does not get better on the next fixture; three in a row
/// with nothing in between is a routing or credential fact about the whole
/// run, and spending the remaining two hundred and forty cases rediscovering
/// it costs an admin an hour and tells them nothing they did not know by case
/// three.
const UNREACHABLE_STREAK: usize = 3;

fn unreachable(score: &EvalCaseScore) -> bool {
    score
        .error
        .as_deref()
        .is_some_and(|e| UNREACHABLE.is_match(e))
}

/// Was this case's failure the DEPLOYMENT rather than the model?
///
/// TWO SHAPES, and the second is the one the traces found. The first is an
/// explicit 429 or an overloaded-gateway 5xx. The second is a request that
/// WENT OUT AND NEVER CAME BACK — every timeout in the first traced sweep
/// read `1 upstream call, still no reply after 60006ms`, which is not a slow
/// model against a tight budget but a lost request, and it says exactly as
/// much about the model as a 429 does, which is nothing. So it retries on the
/// same path: a case that measured nothing must not be scored as if it had.
fn lost_request(score: &EvalCaseScore) -> bool {
    score.timed_out
        && score
            .upstream
            .as_ref()
            .is_some_and(|ups| ups.iter().any(|u| !u.settled))
}

fn rate_limited(score: &EvalCaseScore) -> bool {
    score
        .error
        .as_deref()
        .is_some_and(|e| PRESSURE.is_match(e))
}

fn pressured(score: &EvalCaseScore) -> bool {
    rate_limited(score) || lost_request(score)
}

/// How many more times to ask, given which shape of nothing came back.
fn retries_for(score: &EvalCaseScore) -> usize {
    if rate_limited(score) {
        PRESSURE_RETRIES
    } else {
        TIMEOUT_RETRIES
    }
}

/// A wait a Stop can interrupt. A sweep that ignored the button for twenty
/// seconds of backoff would be the Stop bug again in a smaller costume.
async fn backoff(ms: u64, stopped: &StopPred) {
    let until = now_ms() + ms as i64;
    while now_ms() < until {
        if stopped() {
            return;
        }
        let remaining = (until - now_ms()).max(0) as u64;
        tokio::time::sleep(Duration::from_millis(remaining.min(250))).await;
    }
}

/// THE SWEEP'S WIDTH, and the two things that move it. One object because the
/// three are meaningless apart: a `narrow` with no ceiling to recover toward
/// is the ratchet `RECOVER_AFTER` exists to undo.
///
/// THE WIDTH IS LIVE, not fixed when the pool is built. Lanes are spawned to
/// the CEILING and park below the current width, so a sweep that narrows
/// sheds lanes inside the harness that hit the pressure, and one that
/// recovers picks them back up without waiting for a pool rebuild. (The first
/// version of this read the width once to size the lane array, and narrowing
/// did nothing until the NEXT harness — two comments claimed otherwise, and
/// neither was true of the code under them.)
pub struct Valve {
    started_width: usize,
    inner: Mutex<ValveInner>,
}

struct ValveInner {
    width: usize,
    low: usize,
    narrowed_because: Option<String>,
    calm: usize,
}

impl Valve {
    pub fn new(width: usize) -> Valve {
        Valve {
            started_width: width,
            inner: Mutex::new(ValveInner {
                width,
                low: width,
                narrowed_because: None,
                calm: 0,
            }),
        }
    }

    /// How many cases may be in flight RIGHT NOW. Re-read constantly.
    pub fn width(&self) -> usize {
        self.inner.lock().expect("the valve is not contended").width
    }

    pub fn ceiling(&self) -> usize {
        self.started_width
    }

    /// The provider pushed back. Halves the width.
    pub fn narrow(&self, why: &str) {
        let mut inner = self.inner.lock().expect("the valve is not contended");
        // THE REASON IS RECORDED EVEN WHEN THERE IS NOTHING LEFT TO NARROW. A
        // sweep already at width 1 that is still being rate-limited has
        // learned the most important thing on this page — the deployment
        // cannot serve this run at all right now — and dropping the sentence
        // because the arithmetic had nowhere to go would leave an admin
        // reading unmeasured cases with no reason beside them.
        if inner.narrowed_because.is_none() {
            inner.narrowed_because = Some(why.chars().take(200).collect());
        }
        inner.calm = 0;
        if inner.width <= 1 {
            return;
        }
        inner.width = (inner.width / 2).max(1);
        inner.low = inner.low.min(inner.width);
    }

    /// A case came back. Reopens a lane after `RECOVER_AFTER` in a row.
    pub fn settled(&self) {
        let mut inner = self.inner.lock().expect("the valve is not contended");
        if inner.width >= self.started_width {
            return;
        }
        inner.calm += 1;
        if inner.calm < RECOVER_AFTER {
            return;
        }
        inner.calm = 0;
        inner.width = inner.width.min(self.started_width) + 1;
        inner.low = inner.low.min(inner.width);
    }

    /// The reported record: what was asked, where it ended, and the narrowest
    /// it ever ran.
    pub fn snapshot(&self) -> SweepConcurrency {
        let inner = self.inner.lock().expect("the valve is not contended");
        SweepConcurrency {
            requested: self.started_width,
            ended: inner.width,
            low: inner.low,
            narrowed_because: inner.narrowed_because.clone(),
        }
    }
}

/// How long a parked lane waits before re-checking the width. Short enough
/// that a reopened lane is working again within a case, long enough that six
/// parked lanes are not a spin loop.
const PARK_MS: u64 = 250;

/// Run `items` through `worker`, `valve.width()` at a time, stopping early
/// when `stop` says so. Order of COMPLETION is not order of submission, which
/// is fine: the resume ledger is a set of case keys and every rate is
/// computed over the whole list.
///
/// Lanes are TASKS, parked below the width — see `Valve` for why that has to
/// be live rather than a lane count fixed at spawn time.
async fn pool(
    items: Vec<&'static EvalCase>,
    valve: &Arc<Valve>,
    stop: &StopPred,
    worker: OnCaseWorker,
) {
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lanes = valve.ceiling().min(items.len()).max(1);
    let mut handles = Vec::with_capacity(lanes);
    for lane in 0..lanes {
        let next = next.clone();
        let items = items.clone();
        let valve = valve.clone();
        let stop = stop.clone();
        let worker = worker.clone();
        handles.push(tokio::spawn(async move {
            loop {
                if stop() {
                    return;
                }
                // Checked BEFORE parking, so the last lanes to finish do not
                // sit waiting on a width that will never rise again for a
                // list that is already empty.
                if next.load(std::sync::atomic::Ordering::SeqCst) >= items.len() {
                    return;
                }
                if lane >= valve.width() {
                    tokio::time::sleep(Duration::from_millis(PARK_MS)).await;
                    continue;
                }
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= items.len() {
                    return;
                }
                worker(items[i]).await;
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }
}

/// The pool's worker: one fixture through the whole case-and-retry arc.
type OnCaseWorker = Arc<dyn Fn(&'static EvalCase) -> BoxFut<()> + Send + Sync>;

/// THE CLOCK ONE CASE RACES — sized per harness, not flat.
///
/// A FLAT 60s WAS A SINGLE-CALL BUDGET APPLIED TO MULTI-CALL CASES, and it
/// read as the model's failure. A case is not one model call: `research-search`
/// runs up to three tool rounds, a dry run up to its own `max_turns`, and a
/// JSON harness can add a repair turn on top. The harnesses that timed out
/// most were exactly the ones that call the most, and every one of those
/// timeouts was then charged against the contract rate.
///
/// So the budget is per-turn, multiplied by the turns the harness may
/// actually take. Reasoning models are slow per call and this scales with
/// them; the bound still exists so one hung transport cannot strand a sweep.
pub const PER_TURN_TIMEOUT_MS: u64 = 60_000;

/// Turns a harness may take in one case, for the clock above. Read off the
/// same constants the code paths use, so a raised turn budget cannot silently
/// leave the timeout behind.
pub fn turns_per_case(def: &HarnessDefinition, dry_run: bool, supplied: bool) -> usize {
    // A dry run drives the loop itself; every other case is one model turn.
    //
    // EXCEPT A SUPPLEMENTED ONE. When the platform supplies a capability the
    // model lacks, the harness runs inside the tool-search transport — up to
    // `MAX_TOOL_ROUNDS` search turns plus a closing turn to answer — and a
    // one-turn clock for that whole loop filed real cases as hung requests
    // that were really four-turn jobs.
    let loop_turns = if dry_run {
        turn_budget(def.dry_run.as_ref().and_then(|d| d.max_turns))
    } else if supplied {
        SUPPLIED_TURNS
    } else {
        1
    };
    let repair = match &def.output {
        Output::Json { repair, .. } => repair.unwrap_or(1) as usize,
        _ => 0,
    };
    loop_turns + repair
}

/// Turns the supplement transport may take: `MAX_TOOL_ROUNDS` searching plus
/// one to write the answer. Stated here rather than imported so this file
/// does not depend on a harness definition for its clock — and if that loop
/// grows, this is the number to grow with it.
const SUPPLIED_TURNS: usize = 4;

pub const DEFAULT_CASE_TIMEOUT_MS: u64 = PER_TURN_TIMEOUT_MS;

/// Bounded for the same reason the runner's `raw` is: a drill-down, not an
/// archive, and a model that answers with 200KB of prose must not be able to
/// turn one failed case into a settings row nothing can read.
const DRILLDOWN_CAP: usize = 4_000;

pub fn case_key(harness: &str, name: &str) -> String {
    format!("{harness}::{name}")
}

// ── Skipping ─────────────────────────────────────────────────────────────────

/// WHY THIS CANDIDATE CANNOT BE TESTED ON THIS HARNESS AT ALL, or None when it
/// can. Asked once per harness, before any fixture runs.
///
/// A SKIP IS NOT A KINDNESS AND NOT A PASS. The rule it encodes is narrow on
/// purpose: the harness declares a REQUEST this candidate's transport is
/// documented to refuse, so the call cannot happen and no reply can exist.
/// That is a fact about the pairing, and it is knowable without spending
/// anything.
///
/// EVERY OTHER FAILURE STILL RUNS. In particular a capability floor refusing a
/// model is NOT skipped: the floor refusing IS the tier-2 result, the harness
/// genuinely cannot be trusted on that model, and the score stage already
/// turns the recorded fact into an `unfit` band naming the capability.
/// Skipping it would replace a correct red cell with a shrug.
pub fn harness_skip_reason(
    label: &str,
    tools: Option<ToolPolicy>,
    model: &str,
    own_tool_loop: bool,
    tool_definitions: bool,
) -> Option<String> {
    if tools != Some(ToolPolicy::Own) {
        return None;
    }
    // THE ORDER MATTERS. A fleet persona runs its own loop, so the harness
    // runs as production runs it. A gateway model has no loop of its own but
    // CAN be handed definitions — so the platform supplies the loop and the
    // sandbox, which measures the thing that actually matters: not "can it
    // emit a tool call" but "given these tools and this situation, what did
    // it do". Only a model that can do neither is untestable here.
    if own_tool_loop || tool_definitions {
        return None;
    }
    Some(format!(
        "{label} runs a tool loop, and \"{model}\" can neither run its own nor be handed tool \
         definitions. The sweep did not call the model: nothing here is a measurement of it."
    ))
}

/// The record of a fixture that was never sent. Every measured field is a zero
/// that `score_harness` excludes rather than averages.
fn skipped_case(harness: &str, name: &str, band: EvalBand, reason: &str) -> EvalCaseScore {
    EvalCaseScore {
        harness: harness.into(),
        case: name.into(),
        band,
        skipped: Some(reason.into()),
        contract_held: false,
        first_pass: false,
        repairs: 0,
        answered: false,
        task: TaskVerdict::Unscored,
        task_error: None,
        gap: None,
        findings: 0,
        latency_ms: 0,
        started_at: iso(0),
        wall_ms: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        cost_usd: None,
        estimated: false,
        timed_out: false,
        optimistic: false,
        error: None,
        prompt: None,
        raw: None,
        turns: None,
        calls: None,
        upstream: None,
    }
}

// ── Scoring (pure over recorded cases) ───────────────────────────────────────

/// Nearest-rank percentile. Exact on the small samples a sweep produces — a
/// harness declares two to five fixtures, and an interpolating percentile over
/// three numbers invents a latency nothing measured.
fn percentile(sorted: &[i64], q: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}

fn rate(n: i64, of: i64) -> f64 {
    if of == 0 {
        0.0
    } else {
        n as f64 / of as f64
    }
}

/// A band's pass rate over the cases that were scorable in it, or None when it
/// had none. NONE, never zero: a band with no fixtures has not been failed.
fn band_score(taskable: &[&EvalCaseScore], band: EvalBand) -> Option<f64> {
    let mine: Vec<&&EvalCaseScore> = taskable.iter().filter(|c| c.band == band).collect();
    if mine.is_empty() {
        return None;
    }
    let passed = mine.iter().filter(|c| c.task == TaskVerdict::Pass).count();
    Some(rate(passed as i64, mine.len() as i64))
}

/// Score one harness's cases. Pure, and it takes the METADATA rather than the
/// registry so that a test can score recorded cases without a registry at all.
pub fn score_harness(meta: HarnessMeta, all: &[EvalCaseScore]) -> HarnessScore {
    // THE PARTITION IS THE WHOLE FUNCTION. A skipped case is an absence, not a
    // zero, and every line below counts over `cases` — the ones that ran.
    // Mixing the two would divide by a denominator that includes fixtures
    // nothing asked.
    let skips: Vec<&EvalCaseScore> = all.iter().filter(|c| c.skipped.is_some()).collect();
    let cases: Vec<&EvalCaseScore> = all.iter().filter(|c| c.skipped.is_none()).collect();
    // A TIMEOUT IS NOT A CONTRACT FAILURE, and counting it as one was the same
    // mistake `skipped` was introduced to fix, wearing different clothes. The
    // model never finished answering, so nothing about its contract was
    // observed — the clock ran out, which is a fact about our budget, the
    // provider's latency and our own tool loop. `scored` is the denominator.
    let scored: Vec<&EvalCaseScore> = cases.iter().copied().filter(|c| !c.timed_out).collect();
    let total = scored.len() as i64;
    let first = scored.iter().filter(|c| c.first_pass).count() as i64;
    let held = scored.iter().filter(|c| c.contract_held).count() as i64;
    let failed_first = scored.iter().filter(|c| !c.first_pass).count() as i64;
    let recovered = scored
        .iter()
        .filter(|c| c.contract_held && !c.first_pass)
        .count() as i64;
    // A GAP IS NOT TASKABLE. `task` is already unscored for one, so this falls
    // out — the count and the reasons are carried separately so they reach the
    // people who can fix them instead of vanishing into an absence.
    let gapped: Vec<&EvalCaseScore> = scored.iter().copied().filter(|c| c.gap.is_some()).collect();
    let taskable: Vec<&EvalCaseScore> = scored
        .iter()
        .copied()
        .filter(|c| c.task != TaskVerdict::Unscored)
        .collect();
    let priced: Vec<&EvalCaseScore> = cases.iter().copied().filter(|c| c.cost_usd.is_some()).collect();
    let mut latencies: Vec<i64> = scored.iter().map(|c| c.latency_ms).collect();
    latencies.sort_unstable();
    // Read before `meta` moves into the score below.
    let repairable = meta.repairable;
    let gap_reasons: Vec<String> = {
        let mut seen: HashSet<String> = HashSet::new();
        gapped
            .iter()
            .filter_map(|c| c.gap.clone())
            .filter(|g| !g.is_empty() && seen.insert(g.clone()))
            .collect()
    };
    HarnessScore {
        // `cases` stays every case that RAN, timeouts included, so the count
        // an admin reads still matches the fixtures spent. `scored` is the
        // denominator of the rates, and the gap between them is the timeout
        // count.
        meta,
        cases: cases.len() as i64,
        skipped: skips.len() as i64,
        skip_reason: skips.first().and_then(|c| c.skipped.clone()),
        gaps: gapped.len() as i64,
        gap_reasons,
        scored: scored.len() as i64,
        contract_rate: rate(first, total),
        repair_rate: rate(held, total),
        repair_yield: (failed_first > 0 && repairable).then(|| rate(recovered, failed_first)),
        task_score: (!taskable.is_empty()).then(|| {
            rate(
                taskable
                    .iter()
                    .filter(|c| c.task == TaskVerdict::Pass)
                    .count() as i64,
                taskable.len() as i64,
            )
        }),
        band_scores: BandScores {
            easy: band_score(&taskable, EvalBand::Easy),
            standard: band_score(&taskable, EvalBand::Standard),
            hard: band_score(&taskable, EvalBand::Hard),
        },
        // OVER `scored` LIKE THE REST. A timed-out case produced no reply, so
        // the guard pass never ran on it and it can contribute no findings.
        // Same for `answered_rate`: "did the model answer" is not a question
        // about a case that never got to.
        guard_rate: rate(
            scored.iter().map(|c| c.findings).sum::<i64>(),
            total,
        ),
        answered_rate: rate(
            scored.iter().filter(|c| c.answered).count() as i64,
            total,
        ),
        latency_p50: percentile(&latencies, 0.5),
        latency_p95: percentile(&latencies, 0.95),
        prompt_tokens: cases.iter().map(|c| c.prompt_tokens).sum(),
        completion_tokens: cases.iter().map(|c| c.completion_tokens).sum(),
        cost_usd: (!priced.is_empty()).then(|| {
            priced.iter().filter_map(|c| c.cost_usd).sum::<f64>()
        }),
        estimated: cases.iter().any(|c| c.estimated),
        timeouts: cases.iter().filter(|c| c.timed_out).count() as i64,
        optimistic: cases.iter().filter(|c| c.optimistic).count() as i64,
    }
}

/// Group recorded cases by harness and score each. Order follows `metas`,
/// which is the registry's order — the order Admin shows the harnesses in.
pub fn score_harnesses(metas: &[HarnessMeta], cases: &[EvalCaseScore]) -> Vec<HarnessScore> {
    let mut out = Vec::new();
    for meta in metas {
        let mine: Vec<EvalCaseScore> = cases
            .iter()
            .filter(|c| c.harness == meta.id)
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        out.push(score_harness(meta.clone(), &mine));
    }
    out
}

/// Everything the scorer needs about a harness, from the registry record.
pub fn meta_of(h: &RegisteredHarness) -> HarnessMeta {
    HarnessMeta {
        id: h.def.id.to_string(),
        label: h.def.label.to_string(),
        source: match h.source {
            crate::harness::registry::HarnessSource::Builtin => "builtin".to_string(),
            crate::harness::registry::HarnessSource::App(slug) => format!("app:{slug}"),
            crate::harness::registry::HarnessSource::Custom => "custom".to_string(),
        },
        output_kind: if h.def.output.is_json() { "json" } else { "text" }.to_string(),
        tools: h
            .def
            .tools
            .map(|t| t.as_str())
            .unwrap_or("none")
            .to_string(),
        requires: h.def.requires.iter().map(|r| r.to_string()).collect(),
        verifies: matches!(&h.def.output,
            Output::Json { verify: Some(_), .. } | Output::Text { verify: Some(_), .. }),
        // Mirrors the runner's own repair rule, which is the only thing that
        // decides whether a repair turn happens: JSON output, and a repair
        // count the harness did not zero out.
        repairable: h.def.output.is_json()
            && match &h.def.output {
                Output::Json { repair, .. } => repair.unwrap_or(1) > 0,
                _ => false,
            },
    }
}

// ── Stopping ─────────────────────────────────────────────────────────────────

/// The candidates sweeping in this process, and the ones asked to stop. SETS
/// rather than booleans because sweeps run concurrently; a second sweep of the
/// SAME model is still refused here, because two sweeps interleaving one
/// candidate's cases is the thing the original boolean protected against.
fn sweeping() -> &'static Mutex<HashSet<String>> {
    static S: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    &S
}

fn stop_requested() -> &'static Mutex<HashSet<String>> {
    static S: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    &S
}

/// THE CASE THAT IS RUNNING RIGHT NOW, per candidate.
///
/// WHY IT IS IN MEMORY AND NOT IN THE STATUS ROW. The persisted status is
/// written after every CASE — it is the progress bar and the resume ledger,
/// and that cadence is already the right one for both. A turn-by-turn view
/// needs an update per MODEL TURN, which on a 250-fixture sweep with a
/// six-turn tool loop is nearly two thousand writes to one `app_settings` row.
///
/// WHAT THAT COSTS, said plainly: an instance that did not start the run
/// shows nothing here. That degrades to an empty panel, never to a wrong one,
/// and the completed-case feed beside it is persisted and unaffected.
fn in_flight() -> &'static Mutex<HashMap<String, HashMap<String, Arc<Mutex<InFlightCase>>>>> {
    static M: LazyLock<Mutex<HashMap<String, HashMap<String, Arc<Mutex<InFlightCase>>>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    &M
}

/// The case a sweep is on, as it happens.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlightCase {
    pub harness: String,
    pub case: String,
    pub band: EvalBand,
    /// Epoch ms. The UI turns this into "running for 12s", which is the
    /// number that tells a watcher whether a sweep is working or wedged.
    pub started_at: i64,
    /// Model turns started, and the most this case may take.
    pub turn: i64,
    pub max_turns: i64,
    /// Upstream calls issued, and how many have no reply yet.
    pub calls: i64,
    pub open: i64,
    /// THE CONVERSATION SO FAR, trimmed — what "show the turns currently
    /// testing" actually means. Rebuilt from the request on every turn,
    /// because the request IS the conversation up to that point.
    pub turns: Vec<EvalTurn>,
}

/// What is this candidate doing right now? EMPTY when nothing is, or when the
/// sweep belongs to another instance.
///
/// A LIST, because a sweep runs several cases at once and a panel showing one
/// of four would make three quarters of a working sweep invisible. Oldest
/// first, so the one most likely to be stuck reads first.
pub fn in_flight_for(model: &str) -> Vec<InFlightCase> {
    in_flight()
        .lock()
        .ok()
        .and_then(|map| map.get(model).cloned())
        .map(|slots| {
            let mut list: Vec<InFlightCase> = slots
                .values()
                .filter_map(|c| c.lock().ok().map(|c| c.clone()))
                .collect();
            list.sort_by_key(|c| c.started_at);
            list
        })
        .unwrap_or_default()
}

/// Ask a running sweep to stop. Returns whether one was running to ask.
/// Without a model, asks every sweep in flight to stop.
pub fn stop_eval_sweep(model: Option<&str>) -> bool {
    match model {
        None => {
            let sweeping = sweeping().lock().expect("the sweep set is not contended");
            if sweeping.is_empty() {
                return false;
            }
            let mut asked = stop_requested().lock().expect("the stop set is not contended");
            for m in sweeping.iter() {
                asked.insert(m.clone());
            }
            true
        }
        Some(model) => {
            let sweeping = sweeping().lock().expect("the sweep set is not contended");
            if !sweeping.contains(model) {
                return false;
            }
            drop(sweeping);
            stop_requested()
                .lock()
                .expect("the stop set is not contended")
                .insert(model.to_string());
            true
        }
    }
}

/// Is a sweep running IN THIS PROCESS? A persisted `running` with this false
/// is a sweep a restart interrupted, which is resumable rather than stuck.
pub fn eval_sweep_running(model: Option<&str>) -> bool {
    let sweeping = sweeping().lock().expect("the sweep set is not contended");
    match model {
        None => !sweeping.is_empty(),
        Some(m) => sweeping.contains(m),
    }
}

// ── The driver ───────────────────────────────────────────────────────────────

/// What the recording transport accumulates over one case, and what the row
/// capture holds.
#[derive(Default)]
struct CaseRun {
    row: Option<HarnessRunRow>,
    prompt: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    estimated: bool,
    timed_out: bool,
    /// EVERY UPSTREAM CALL THIS CASE MADE, in order.
    upstream: Vec<UpstreamAttempt>,
}

/// A call still open when the case was killed has no duration of its own yet.
/// Give it the time it had actually been waiting, so the sentence below can
/// say "still had no reply after 59.4s" rather than "0ms".
fn settle_open(calls: &[UpstreamAttempt], started_at: i64, now: i64) -> Vec<UpstreamAttempt> {
    calls
        .iter()
        .map(|c| {
            if c.settled {
                c.clone()
            } else {
                UpstreamAttempt {
                    ms: now - started_at,
                    ..c.clone()
                }
            }
        })
        .collect()
}

/// THE SENTENCE A TIMEOUT SHOULD HAVE BEEN. Names what the budget was actually
/// spent on, which is the whole question an admin has when a model they know
/// to be fast times out.
fn timeout_detail(case_ms: u64, calls: &[UpstreamAttempt]) -> String {
    if calls.is_empty() {
        // The case never got a request out. That is not the model: it is
        // route resolution, the capability floor, key resolution or the
        // provider catalog — all of which happen before a token is spent and
        // all of which can block.
        return format!(
            "the case did not finish inside {case_ms}ms and never made an upstream call at all; \
             the time went somewhere before the request (route resolution, the endpoint key, or \
             the provider catalog), not to the model"
        );
    }
    let open: Vec<&UpstreamAttempt> = calls.iter().filter(|c| !c.settled).collect();
    let done: Vec<&UpstreamAttempt> = calls.iter().filter(|c| c.settled).collect();
    let mut parts = vec![format!(
        "the case did not finish inside {case_ms}ms after {} upstream call(s)",
        calls.len()
    )];
    if !done.is_empty() {
        let listed = done
            .iter()
            .map(|c| format!("{}ms{}", c.ms, if c.error.is_some() { " error" } else { "" }))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("{} came back ({listed})", done.len()));
    }
    if !open.is_empty() {
        let longest = open.iter().map(|c| c.ms).max().unwrap_or(0);
        parts.push(format!(
            "{} still had no reply after {longest}ms",
            open.len()
        ));
    }
    if let Some(last) = calls.iter().filter(|c| c.error.is_some()).next_back() {
        parts.push(format!("last error: {}", last.error.clone().unwrap_or_default()));
    }
    parts.join("; ")
}

/// THE TRANSPORT THE SWEEP WRAPS AROUND THE REAL ONE: it changes nothing about
/// the call and records the prompt and the token counts, which no other seam
/// can see. The runner's result carries neither by design — the drill-down
/// needs the prompt and the cost line needs the tokens, and widening the
/// runner's result type for a benchmark would put benchmark concerns in the
/// hot path.
fn recording_transport(
    base: TransportFn,
    into: Arc<Mutex<CaseRun>>,
    live: Option<Arc<Mutex<InFlightCase>>>,
    now: NowFn,
) -> TransportFn {
    Arc::new(move |req: TransportRequest| {
        let base = base.clone();
        let into = into.clone();
        let live = live.clone();
        let now = now.clone();
        Box::pin(async move {
            {
                let mut capture = into.lock().expect("one case, one capture");
                capture.prompt = req
                    .messages
                    .iter()
                    .map(|m| format!("{}: {}", m.role.as_str(), m.content))
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            if let Some(live) = &live {
                let mut live = live.lock().expect("the live view is not contended");
                live.turn += 1;
                // THE REQUEST IS THE CONVERSATION. A tool loop hands the whole
                // transcript back down on every turn, so publishing the
                // request's messages gives a live view that grows a turn at a
                // time without the loop having to report anything.
                live.turns = record_turns(&req.messages).unwrap_or_else(|| {
                    req.messages
                        .iter()
                        .map(|m| EvalTurn {
                            role: m.role.as_str().to_string(),
                            content: utf16_truncate(&m.content, LIVE_TURN_CAP),
                            tool_calls: None,
                        })
                        .collect()
                });
                live.calls += 1;
                live.open += 1;
            }
            // PUSHED BEFORE THE AWAIT, so a call that never comes back is still
            // in the list when the case is killed. Recording on completion
            // would make the one attempt that matters — the one that hung —
            // the only one invisible.
            let at = now();
            let index = {
                let mut capture = into.lock().expect("one case, one capture");
                capture.upstream.push(UpstreamAttempt {
                    ms: 0,
                    settled: false,
                    error: None,
                });
                capture.upstream.len() - 1
            };
            let reply = base(req).await;
            {
                let mut capture = into.lock().expect("one case, one capture");
                let attempt = capture
                    .upstream
                    .get_mut(index)
                    .expect("the attempt was pushed before the await");
                attempt.ms = now() - at;
                attempt.settled = true;
                if let Err(err) = &reply {
                    attempt.error = Some(err.chars().take(300).collect());
                }
            }
            if let Some(live) = &live {
                let mut live = live.lock().expect("the live view is not contended");
                live.open -= 1;
                if let Ok(reply) = &reply
                    && !reply.text.trim().is_empty()
                {
                    // The reply, appended, so the panel shows what came back
                    // rather than only what went out.
                    live.turns.push(EvalTurn {
                        role: "assistant".into(),
                        content: utf16_truncate(&reply.text, LIVE_TURN_CAP),
                        tool_calls: None,
                    });
                }
            }
            let reply = reply?;
            {
                let mut capture = into.lock().expect("one case, one capture");
                match reply.usage {
                    Some(u) => {
                        capture.prompt_tokens += u.prompt_tokens;
                        capture.completion_tokens += u.completion_tokens;
                    }
                    None => {
                        // The same chars/4 fallback the token ledger uses, from
                        // the same helper — a second estimator would give the
                        // fitness page and the invoice two different token
                        // counts for one call. JS `.length` is UTF-16 units.
                        capture.prompt_tokens +=
                            estimate_tokens(capture.prompt.encode_utf16().count());
                        capture.completion_tokens +=
                            estimate_tokens(reply.text.encode_utf16().count());
                        capture.estimated = true;
                    }
                }
            }
            Ok(reply)
        })
    })
}

/// One turn in the LIVE view, bounded harder than the archived one: it is
/// re-sent on every poll while a case runs, so a work-session prompt at full
/// length would be shipped a dozen times per case for no reading anyone does.
const LIVE_TURN_CAP: usize = 600;

/// A tool result the model saw, bounded harder than the prose is. The loop
/// itself truncates at 8 000 characters before handing one back; this is the
/// archive's share of that, and a `get_ticket` payload is the reason it
/// exists.
const TOOL_CAP: usize = 1_200;
/// A tool RESULT, kept shorter still — see `record_calls`.
const RESULT_CAP: usize = 600;

/// JS `s.slice(0, n)` — a UTF-16-unit cut. Rust strings are UTF-8, so the
/// cut walks code units and stops before splitting a surrogate pair (a lone
/// trailing surrogate would make the archived JSON unparseable, which a
/// drill-down must never do). For BMP text the two spellings are identical.
fn utf16_truncate(s: &str, units: usize) -> String {
    let mut out = String::with_capacity(s.len().min(units * 4));
    let mut count = 0usize;
    for ch in s.chars() {
        let width = ch.len_utf16();
        if count + width > units {
            return out;
        }
        count += width;
        out.push(ch);
    }
    out
}

fn cap(text: Option<&str>) -> Option<String> {
    text.map(|t| utf16_truncate(t, DRILLDOWN_CAP))
}

/// THE TRANSCRIPT, FLATTENED FOR THE ARCHIVE.
///
/// A single-turn case returns None: `prompt` and `raw` already carry the whole
/// exchange, and writing it twice per case would double the size of a settings
/// row for no reading anyone would do.
fn record_turns(messages: &[crate::harness::define::Message]) -> Option<Vec<EvalTurn>> {
    if messages.len() <= 2 {
        return None;
    }
    Some(
        messages
            .iter()
            .map(|m| EvalTurn {
                role: m.role.as_str().to_string(),
                content: utf16_truncate(&m.content, TOOL_CAP),
                tool_calls: (!m.tool_calls.is_empty()).then(|| {
                    m.tool_calls.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
                }),
            })
            .collect(),
    )
}

/// WHAT THE MODEL DID, kept for every dry-run case — but not at full weight.
///
/// `with_results` is false for a case that passed cleanly, and that is a size
/// decision with a reading behind it. The NAMES and the ARGUMENTS are what
/// every behavioural fixture asserts over and what an admin compares between
/// two models, and they are a few hundred bytes; the RESULTS are a
/// `get_ticket` payload each and are the whole weight. On a case that failed,
/// the result is often the explanation (the tool refused, and the model
/// carried on anyway), so it stays. On a case that passed there is nothing to
/// explain.
fn record_calls(surface: Option<&Arc<Mutex<CaseSurface>>>, with_results: bool) -> Option<Vec<EvalToolCall>> {
    let surface = surface?;
    let calls = surface.lock().expect("one case, one sandbox").recorded();
    Some(
        calls
            .iter()
            .map(|c| EvalToolCall {
                tool: c.tool.clone(),
                args: utf16_truncate(&c.args.to_string(), TOOL_CAP),
                result: (with_results && c.result.is_some()).then(|| {
                    utf16_truncate(&c.result.clone().unwrap_or(Value::Null).to_string(), RESULT_CAP)
                }),
                error: c.error.clone(),
            })
            .collect(),
    )
}

/// How often a case in flight asks whether it has been stopped.
///
/// STOP USED TO BE HONORED ONLY BETWEEN CASES, and that is why the button read
/// as broken. A dry run is budgeted `PER_TURN_TIMEOUT_MS × turns_per_case` —
/// seven minutes for a work-session fixture — so an admin who pressed Stop
/// watched nothing happen for minutes and pressed it again.
///
/// Half a second is nothing against a model call and is instant to a person.
const STOP_POLL_MS: u64 = 500;

/// How often the sweep re-reads the PERSISTED stop request. One second, so a
/// Stop pressed against another worker reaches the running case about as fast
/// as one pressed against this one.
const STOP_WATCH_MS: u64 = 1_000;

/// The three sandbox surfaces a dry run can run against, narrowed to what the
/// sweep and the loop both need. The loop cares about `tools` and `dispatch`
/// (the `DispatchSandbox` impl); the sweep's fixtures additionally read the
/// call log and the world, and the archive reads the same log with results.
enum CaseSurface {
    /// Talaria's own toolkit over an in-memory world.
    Toolkit(Sandbox),
    /// A coding harness: files and a test runner.
    Files(WorkbenchSandbox),
    /// A credential surface: a shell and outbound HTTP.
    Credentials(CredentialSandbox),
}

/// One recorded sandbox call, whichever surface made it — the two fields a
/// `CheckCall` builds from and the four the archive keeps.
struct RecordedCall {
    tool: String,
    args: Value,
    result: Option<Value>,
    error: Option<String>,
}

impl CaseSurface {
    fn recorded(&self) -> Vec<RecordedCall> {
        let map = |c: &SandboxCall| RecordedCall {
            tool: c.tool.clone(),
            args: c.args.clone(),
            result: c.result.clone(),
            error: c.error.clone(),
        };
        match self {
            CaseSurface::Toolkit(s) => s.calls.iter().map(map).collect(),
            CaseSurface::Files(s) => s.calls.iter().map(map).collect(),
            // The credential log keeps `spent` too — which boundary a call
            // resolved on — and no fixture asserts on it through the context;
            // the def's own helpers read their sandbox directly.
            CaseSurface::Credentials(s) => s
                .calls
                .iter()
                .map(|c| RecordedCall {
                    tool: c.tool.clone(),
                    args: c.args.clone(),
                    result: c.result.clone(),
                    error: c.error.clone(),
                })
                .collect(),
        }
    }

    /// The world slot a fixture's `CheckCtx.world` reads. Only the toolkit has
    /// a Talaria world; the file surface stands in `{ files, failure }`, and
    /// the credential surface's world is the grants themselves (the values
    /// stay here — a fixture reads them, the model never does).
    fn world_value(&self) -> Value {
        match self {
            CaseSurface::Toolkit(s) => s.world.to_value(),
            CaseSurface::Files(s) => s.world_as_value(),
            CaseSurface::Credentials(s) => json!({
                "granted": s.world.granted.iter().map(|g| json!({
                    "handle": g.handle,
                    "value": g.value,
                    "accepts": g.accepts,
                })).collect::<Vec<_>>(),
            }),
        }
    }
}

impl DispatchSandbox for CaseSurface {
    fn tools(&self) -> Vec<crate::harness::transport::ToolDefinition> {
        match self {
            CaseSurface::Toolkit(s) => Sandbox::tool_definitions(s),
            CaseSurface::Files(s) => WorkbenchSandbox::tools(s),
            CaseSurface::Credentials(s) => CredentialSandbox::tools(s),
        }
    }
    fn dispatch(&mut self, name: &str, args_json: &str) -> DispatchResult {
        match self {
            CaseSurface::Toolkit(s) => Sandbox::dispatch(s, name, args_json),
            CaseSurface::Files(s) => WorkbenchSandbox::dispatch(s, name, args_json),
            CaseSurface::Credentials(s) => CredentialSandbox::dispatch(s, name, args_json),
        }
    }
}

/// The runner's dep set with the sweep's three edges layered on. Wholesale,
/// because that is the Rust runner's one spelling of a dep override — see
/// `RunContext.deps`.
fn sweep_harness_deps(
    base: &HarnessDeps,
    transport: TransportFn,
    record_run: RecordRunFn,
    record_findings: RecordFindingsFn,
) -> HarnessDeps {
    HarnessDeps {
        resolve_model: base.resolve_model.clone(),
        slot_effort: base.slot_effort.clone(),
        routing: base.routing.clone(),
        persona_keys: base.persona_keys.clone(),
        missing_capabilities: base.missing_capabilities.clone(),
        capabilities: base.capabilities.clone(),
        reach: base.reach.clone(),
        transport,
        guard_config: base.guard_config.clone(),
        guard_text: base.guard_text.clone(),
        record_findings,
        record_run,
        now: base.now.clone(),
    }
}

enum CaseOutcome {
    Done(Result<crate::harness::run::HarnessResult, String>),
    Stopped,
    TimedOut,
}

/// One fixture, replayed once against the candidate. None means the case was
/// CANCELLED mid-flight — nothing is recorded, the fixture stays pending, and
/// a resume picks it up rather than inheriting a failure that never happened.
#[allow(clippy::too_many_arguments)]
async fn run_one_case(
    state: &AppState,
    def: &'static HarnessDefinition,
    fixture: &'static EvalCase,
    model: &str,
    deps: &Arc<EvalDeps>,
    base_harness_deps: &Arc<HarnessDeps>,
    timeout_ms: u64,
    dry_run: bool,
    stopped: &StopPred,
) -> Option<EvalCaseScore> {
    let started_at = (deps.now)();
    let capture = Arc::new(Mutex::new(CaseRun::default()));
    // PUBLISHED FOR THE LIVE PANEL, and cleared in every exit path below — a
    // stale "running now" left behind by a stopped case is worse than none,
    // because it makes a finished sweep look like a wedged one.
    let live = Arc::new(Mutex::new(InFlightCase {
        harness: def.id.to_string(),
        case: fixture.name.to_string(),
        band: fixture.band,
        started_at: started_at,
        turn: 0,
        max_turns: turns_per_case(def, dry_run, false) as i64,
        calls: 0,
        open: 0,
        turns: Vec::new(),
    }));
    let key = case_key(def.id, fixture.name);
    {
        let mut flights = in_flight().lock().expect("the in-flight map is not contended");
        flights
            .entry(model.to_string())
            .or_default()
            .insert(key.clone(), live.clone());
    }
    // CANCELLED IS NOT FAILED, and conflating them would be the worse bug of
    // the two: writing a cancelled case into the resume ledger would mark the
    // fixture done, skip it on resume, and leave the model carrying a phantom
    // failure it was never actually given a chance at.
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ── SUPPLEMENT WHAT THE MODEL LACKS AND THE DEPLOYMENT HAS ──────────────
    // `RoleFloor.suppliable` already lets the run PROCEED when a capability is
    // reachable through a registered tool — that is how `research-search`
    // avoids refusing a model that cannot browse. But an ordinary transport
    // carries no tool, so the model would answer from memory and the fixture
    // would fail it for having no sources. Production does not do that: the
    // research stages pick the tool-search transport for exactly this case,
    // and so does the sweep — the benchmark measures what an admin actually
    // assigns, a model running inside Talaria with the tools this org
    // registered, not the bare weights.
    let supplier = match (def.floor.suppliable.first(), &deps.supplier) {
        (Some(capability), Some(supplier)) => supplier(capability.to_string()).await,
        _ => None,
    };
    // WHAT THE SUPPLIED SEARCH TOOL ACTUALLY RETURNED, across every call this
    // case made. Empty after a run that called the tool means our search found
    // nothing — our gap, not the model's failure.
    let sources: SearchSink = Arc::new(Mutex::new(Vec::new()));

    // AN ISOLATED TALARIA, ONE PER CASE, only for a harness whose feature is
    // the tool loop; every other harness gets none and its fixtures see the
    // empty context. Per case rather than per harness because two fixtures
    // sharing a mutable board would make the second one's assertions depend
    // on the first one's model.
    let workspace = dry_run
        .then(|| def.dry_run.as_ref().and_then(|d| d.workspace.as_ref()))
        .flatten()
        .map(|w| w(&fixture.input));
    let credentials = dry_run
        .then(|| def.dry_run.as_ref().and_then(|d| d.credentials.as_ref()))
        .flatten()
        .map(|c| c(&fixture.input));
    let dry_world = dry_run
        .then(|| def.dry_run.as_ref().and_then(|d| d.world.as_ref()))
        .flatten()
        .map(|w| w(&fixture.input));
    // WHICH SURFACE. `workspace` is a CODING harness (files and a test
    // runner); `credentials` is a harness whose subject is spending a
    // credential the model cannot read; everything else gets Talaria's own
    // toolkit. A harness has exactly one of them.
    let surface: Option<Arc<Mutex<CaseSurface>>> = if !dry_run {
        None
    } else if let Some(spec) = workspace {
        Some(Arc::new(Mutex::new(CaseSurface::Files(
            WorkbenchSandbox::new(spec),
        ))))
    } else if let Some(spec) = credentials {
        Some(Arc::new(Mutex::new(CaseSurface::Credentials(
            CredentialSandbox::new(spec),
        ))))
    } else {
        let opts = SandboxOptions {
            tools: def
                .dry_run
                .as_ref()
                .map(|d| d.tools.iter().map(|t| t.to_string()).collect()),
            world: dry_world.unwrap_or(Value::Null),
        };
        Some(Arc::new(Mutex::new(CaseSurface::Toolkit(Sandbox::new(
            opts,
        )))))
    };
    let dry: Arc<Mutex<Option<DryRunResult>>> = Arc::new(Mutex::new(None));
    let base: TransportFn = base_harness_deps.transport.clone();

    let stack: TransportFn = if let Some(surface) = &surface {
        sandbox_transport(
            surface.clone(),
            base,
            Some(dry.clone()),
            turn_budget(def.dry_run.as_ref().and_then(|d| d.max_turns)),
        )
    } else if let Some(supplier) = &supplier {
        // The same transport production uses, driving the same tool. THE SINK
        // IS KEPT, not thrown away: it is the only evidence of whether the
        // deployment's search actually FOUND anything, and that is the
        // difference between a model that answered badly and a search backend
        // that returned nothing for it to answer from.
        tool_search_transport(
            state.clone(),
            format!("fitness:{}", def.id),
            sources.clone(),
            supplier.clone(),
            ToolSearchDeps {
                base: Some(base),
                call_tool: deps.search_tool.clone(),
            },
        )
    } else {
        base
    };

    // The row the runner writes IS the predicate this file scores. Captured
    // and NOT forwarded to the real recorder: a sweep must not file into the
    // table it is being compared against. Same argument, and stronger, for
    // the findings half — the guard pass still runs; only the filing is
    // suppressed.
    let capture_for_row = capture.clone();
    let record_run: RecordRunFn = Arc::new(move |row: HarnessRunRow| {
        let capture = capture_for_row.clone();
        Box::pin(async move {
            capture.lock().expect("one case, one capture").row = Some(row);
        })
    });
    let record_findings: RecordFindingsFn = noop_record_findings();
    let harness_deps = Arc::new(sweep_harness_deps(
        base_harness_deps,
        recording_transport(stack, capture.clone(), Some(live.clone()), deps.now.clone()),
        record_run,
        record_findings,
    ));

    let ctx = crate::harness::run::RunContext {
        caller: format!("fitness:{}", def.id),
        model: Some(model.to_string()),
        deps: Some(harness_deps),
        ..Default::default()
    };
    let input = fixture.input.clone();
    let state_for_work = state.clone();
    let work = async move {
        run_harness(&state_for_work, def, &input, ctx)
            .await
            .map_err(|e| e.0)
    };

    // SIZED TO WHAT THIS CASE MAY DO, not to a flat single-call figure — see
    // `turns_per_case`. The caller's budget is the PER-TURN allowance.
    let case_ms = timeout_ms * turns_per_case(def, dry_run, supplier.is_some()) as u64;
    let stop_poll = {
        let stopped = stopped.clone();
        let cancelled = cancelled.clone();
        async move {
            loop {
                if stopped() {
                    cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(STOP_POLL_MS)).await;
            }
        }
    };
    let outcome = tokio::select! {
        reply = work => CaseOutcome::Done(reply),
        _ = stop_poll => CaseOutcome::Stopped,
        _ = tokio::time::sleep(Duration::from_millis(case_ms)) => {
            capture.lock().expect("one case, one capture").timed_out = true;
            CaseOutcome::TimedOut
        }
    };
    {
        let mut flights = in_flight().lock().expect("the in-flight map is not contended");
        if let Some(slots) = flights.get_mut(model) {
            slots.remove(&key);
        }
    }
    if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }

    let done = matches!(outcome, CaseOutcome::Done(_));
    // `on_failure: throw` harnesses throw out of the runner by declaration.
    // A benchmark that let that escape would let one harness's failure
    // policy end the sweep. The row is already captured — the runner
    // writes it before it throws, precisely so a throwing harness stays
    // visible.
    let threw = match &outcome {
        CaseOutcome::Done(Err(message)) => Some(message.clone()),
        _ => None,
    };
    let result = match outcome {
        CaseOutcome::Done(Ok(r)) => Some(r),
        _ => None,
    };
    // THE CAPTURE IS SNAPSHOT AND RELEASED HERE. The price edge below can
    // await, and a MutexGuard is not Send — one held across that await would
    // make the whole case future unspawnable, which is exactly the kind of
    // constraint a lock should never leak into a driver's shape.
    let (row, prompt, prompt_tokens, completion_tokens, estimated, timed_out, upstream) = {
        let guard = capture.lock().expect("one case, one capture");
        (
            guard.row.clone(),
            guard.prompt.clone(),
            guard.prompt_tokens,
            guard.completion_tokens,
            guard.estimated,
            guard.timed_out,
            guard.upstream.clone(),
        )
    };
    // THE CONTRACT, from the row the runner wrote — never re-derived here.
    let contract_held = row
        .as_ref()
        .map(|r| r.schema_valid)
        .or(result.as_ref().map(|r| r.schema_valid))
        .unwrap_or(false);
    let repairs = row
        .as_ref()
        .map(|r| r.repairs as i64)
        .or(result.as_ref().map(|r| r.repairs as i64))
        .unwrap_or(0);
    let first_pass = contract_held && repairs == 0;

    // WHAT THE MODEL DID, for a fixture that grades behaviour rather than
    // prose. The empty context for everything else, so a single-shot fixture
    // reaching for its calls sees an honest empty list.
    let (ctx_calls, ctx_world, ctx_exhausted) = match &surface {
        Some(surface) => {
            let guard = surface.lock().expect("one case, one sandbox");
            (
                guard
                    .recorded()
                    .iter()
                    .map(|c| crate::harness::define::CheckCall {
                        tool: c.tool.clone(),
                        errored: c.error.is_some(),
                        args: c.args.clone(),
                    })
                    .collect::<Vec<_>>(),
                Some(guard.world_value()),
                dry.lock()
                    .expect("one case, one dry result")
                    .as_ref()
                    .map(|d| d.exhausted)
                    .unwrap_or(false),
            )
        }
        None => (Vec::new(), None, false),
    };
    let eval_context = CheckCtx {
        calls: ctx_calls,
        world: ctx_world,
        exhausted: ctx_exhausted,
    };

    let mut task = TaskVerdict::Unscored;
    let mut task_error: Option<String> = None;
    // THE HARNESS'S OWN GAP, kept apart from the model's score. A fixture that
    // could not fairly ask its question reports it here and the case is scored
    // unscored, exactly as a skip is: "we did not give it what the job
    // needed" is not "it answered badly", and attributing one to the other
    // measures our fixture and calls it a capability.
    let mut gap: Option<String> = None;
    if contract_held
        && let Some(result) = &result
        && let Some(value) = &result.value
    {
        let verdict = (fixture.check)(value, &eval_context);
        match is_gap(&verdict) {
            Some(sentence) => gap = Some(sentence.to_string()),
            None => {
                if let CheckResult::Fail(sentence) = verdict {
                    task_error = Some(sentence);
                }
            }
        }
        // WE CUT IT OFF, THEN JUDGED THE RESULT. An exhausted dry run is a
        // model still working when the loop's budget ran out; grading the
        // half-finished state charges our budget to the model. IT IS SAFE AS A
        // BLANKET RULE, which is not obvious: a fixture that measures
        // RESTRAINT is satisfied by a model that called almost nothing, and a
        // model that called almost nothing cannot have exhausted a six-turn
        // loop. So the fixtures this would wrongly excuse are the ones it
        // cannot reach, and the failure it does excuse is real every time.
        if gap.is_none()
            && task_error.is_some()
            && eval_context.exhausted
        {
            gap = Some(format!(
                "the model was still working when the loop's {}-turn budget ran out, and the \
                 assertion then judged unfinished work (\"{}\"). Raise this harness's \
                 dryRun.maxTurns or ask the fixture something a bounded loop can answer.",
                turns_per_case(def, dry_run, false),
                task_error.clone().unwrap_or_default()
            ));
            task_error = None;
        }
        // OUR SEARCH FOUND NOTHING, so the fixture could not fairly ask its
        // question. The model did exactly what the harness asks — decline to
        // state from these results — and the sweep would record a task failure
        // for it: the better the model behaves, the worse it scores. THE
        // SIGNAL IS THE SINK, not the prose: the tool-search transport refuses
        // when the model never called the tool at all (that IS a model
        // failure), so reaching here with an empty sink means the tool was
        // called and returned nothing citable.
        if gap.is_none()
            && task_error.is_some()
            && supplier.is_some()
            && sources.lock().expect("one case, one sink").is_empty()
        {
            gap = Some(format!(
                "the model called \"{}\" and this deployment's search returned nothing citable, \
                 so the fixture judged an answer written from no sources (\"{}\"). Fix the search \
                 backend or ask this fixture something the installed engines can find.",
                supplier.as_ref().map(|s| s.tool.clone()).unwrap_or_default(),
                task_error.clone().unwrap_or_default()
            ));
            task_error = None;
        }
        task = if gap.is_some() {
            TaskVerdict::Unscored
        } else if task_error.is_none() {
            TaskVerdict::Pass
        } else {
            TaskVerdict::Fail
        };
    }

    // THE FLOOR DECLINED TO ASK — no question reached the model, so nothing
    // here is a fact about it. Recorded as a SKIP, exactly like a harness this
    // candidate's transport cannot drive. It used to be a failure, charged to
    // the candidate — five fixtures refusing with a sentence the model never
    // saw, recorded as five wrong answers.
    if result.as_ref().map(|r| r.refused).unwrap_or(false) {
        let reason = result
            .as_ref()
            .and_then(|r| r.error.clone())
            .unwrap_or_else(|| {
                "the capability floor refused this model, so the fixture was never asked".into()
            });
        return Some(skipped_case(def.id, fixture.name, fixture.band, &reason));
    }

    // A GAP IS NOT CLEAN. `clean` decides whether the drill-down is kept or
    // dropped, and a gap is exactly the case where it matters most: the
    // fixture is telling US it could not fairly ask its question, and the
    // first thing whoever owns that fixture needs is what was actually sent
    // and what came back.
    let clean = done && contract_held && task != TaskVerdict::Fail && gap.is_none();
    let tokens = prompt_tokens + completion_tokens;
    let cost_usd = if tokens > 0 {
        (deps.price)(model.to_string(), prompt_tokens, completion_tokens).await
    } else {
        None
    };
    let now = (deps.now)();
    let score = EvalCaseScore {
        harness: def.id.to_string(),
        case: fixture.name.to_string(),
        band: fixture.band,
        skipped: None,
        contract_held,
        first_pass,
        repairs,
        answered: result.as_ref().map(|r| r.answered).unwrap_or(false),
        task,
        task_error,
        gap,
        findings: row.as_ref().map(|r| r.findings).unwrap_or(0),
        latency_ms: row.as_ref().map(|r| r.latency_ms).unwrap_or(0),
        started_at: iso(started_at),
        wall_ms: now - started_at,
        prompt_tokens,
        completion_tokens,
        cost_usd,
        estimated,
        timed_out,
        optimistic: contract_held && task == TaskVerdict::Fail,
        error: if timed_out {
            Some(timeout_detail(case_ms, &settle_open(&upstream, started_at, now)))
        } else {
            threw.or(row.as_ref().and_then(|r| r.error.clone()))
                .or(result.as_ref().and_then(|r| r.error.clone()))
        },
        prompt: (!clean).then(|| cap(Some(&prompt))).flatten(),
        raw: (!clean)
            .then(|| cap(result.as_ref().and_then(|r| r.raw.as_deref())))
            .flatten(),
        // THE TRANSCRIPT FOLLOWS THE SAME RULE AS THE PROMPT. THE CALL LOG
        // DOES NOT: it is small, it is what every behavioural fixture asserts
        // over, and comparing two models on one fixture means comparing the
        // two lists — available only on failure would mean the comparison
        // worth making is the one you cannot see.
        turns: (!clean).then(|| record_turns(dry.lock().expect("one case, one dry result").as_ref().map(|d| d.messages.as_slice()).unwrap_or(&[]))).flatten(),
        calls: record_calls(surface.as_ref(), !clean),
        upstream: (!clean).then(|| settle_open(&upstream, started_at, now)),
    };
    Some(score)
}

// A findings recorder that files nothing — see the file header for why the
// sweep must not move the number it is compared against.
fn noop_record_findings() -> RecordFindingsFn {
    Arc::new(|_findings, _meta| Box::pin(async {}))
}

/// THE SWEEP: every fixture in the registry, replayed against one candidate.
///
/// RESUMABLE. The persisted status carries the scored cases, so a sweep an
/// admin stopped — or a deploy interrupted — restarts where it left off rather
/// than re-buying seventy calls. Resume is only ever within ONE candidate.
///
/// BOUNDED. Every case races a wall clock and a case that loses is recorded
/// `timed_out` and left behind. No single harness can strand the sweep.
///
/// LANED, NOT SEQUENTIAL. A parallel sweep's `latencyP50` includes queueing at
/// the provider, which is why the width is RECORDED and the page labels the
/// number with it; and a self-hosted 14B behind one GPU answers width with
/// rate-limit errors, which is what the valve is for.
pub async fn run_eval_sweep(state: &AppState, model: &str, opts: EvalOptions) -> EvalSweep {
    let deps = opts
        .deps
        .clone()
        .unwrap_or_else(|| Arc::new(real_deps(state)));

    // One sweep at a time PER CANDIDATE. A second concurrent sweep of the same
    // model would interleave two runs' cases into one checkpoint. The caller
    // gets the RUNNING sweep's progress back rather than an error — the second
    // press of a Test button means "show me the run" — with no harness scores
    // on it, because scoring a half-finished sweep would print a contract rate
    // over the cases that happened to be done.
    if sweeping()
        .lock()
        .expect("the sweep set is not contended")
        .contains(model)
    {
        let status = (deps.read_status)(model.to_string()).await;
        return sweep_of(status, Vec::new(), Vec::new(), false, None, Vec::new());
    }
    sweeping()
        .lock()
        .expect("the sweep set is not contended")
        .insert(model.to_string());
    stop_requested()
        .lock()
        .expect("the stop set is not contended")
        .remove(model);

    let result = sweep_body(state, model, &opts, &deps).await;
    let sweep = match result {
        Ok(sweep) => sweep,
        Err(message) => {
            // Same shape as the reindex pair: the failure lands in the status
            // rather than escaping to a route handler, because the admin who
            // pressed the button is watching this row and not a stack trace.
            let prior = (deps.read_status)(model.to_string()).await;
            let failed = EvalSweepStatus {
                state: EvalSweepState::Error,
                model: Some(model.to_string()),
                finished_at: Some(iso((deps.now)())),
                harness: None,
                error: Some(message),
                ..prior
            };
            (deps.write_status)(failed.clone()).await;
            sweep_of(failed, Vec::new(), Vec::new(), false, None, Vec::new())
        }
    };

    // THE FINALLY — every path. A "running now" that outlives its sweep makes
    // a finished run look wedged, which is exactly the confusion this panel
    // exists to remove.
    if let Some(prune) = &opts.archive_prune {
        let _ = prune(model.to_string()).await;
    }
    sweeping()
        .lock()
        .expect("the sweep set is not contended")
        .remove(model);
    stop_requested()
        .lock()
        .expect("the stop set is not contended")
        .remove(model);
    in_flight()
        .lock()
        .expect("the in-flight map is not contended")
        .remove(model);
    sweep
}

/// The watcher that keeps `externally_stopped` fresh, so the SYNCHRONOUS
/// predicate a case polls is never more than a second stale. The persisted
/// request is the only channel a Stop pressed against another worker has.
fn spawn_stop_watcher(
    opts: &EvalOptions,
    model: &str,
    externally_stopped: Arc<std::sync::atomic::AtomicBool>,
) -> Option<tokio::task::JoinHandle<()>> {
    let should_stop = opts.should_stop.clone()?;
    let model = model.to_string();
    Some(tokio::spawn(async move {
        loop {
            {
                let ask = should_stop(model.clone()).await;
                // LATCHED, exactly as the TS's `externallyStopped ||` latches:
                // a Stop that landed once stays landed for the rest of the run.
                if ask {
                    externally_stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            tokio::time::sleep(Duration::from_millis(STOP_WATCH_MS)).await;
        }
    }))
}

async fn sweep_body(
    state: &AppState,
    model: &str,
    opts: &EvalOptions,
    deps: &Arc<EvalDeps>,
) -> Result<EvalSweep, String> {
    let externally_stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = spawn_stop_watcher(opts, model, externally_stopped.clone());
    let out = sweep_inner(state, model, opts, deps, externally_stopped).await;
    // THE WATCHER STOPS ON EVERY PATH. A leaked one keeps polling the
    // persisted stop request once a second for the life of the process.
    if let Some(w) = watcher {
        w.abort();
    }
    out
}

async fn sweep_inner(
    state: &AppState,
    model: &str,
    opts: &EvalOptions,
    deps: &Arc<EvalDeps>,
    externally_stopped: Arc<std::sync::atomic::AtomicBool>,
) -> Result<EvalSweep, String> {
    let timeout_ms = opts.case_timeout_ms.unwrap_or(DEFAULT_CASE_TIMEOUT_MS);
    // True the instant either half of the request lands. Sync, so a case can
    // ask it from a timer.
    let stopped_now: StopPred = {
        let externally_stopped = externally_stopped.clone();
        let local: Arc<dyn Fn() -> bool + Send + Sync> = {
            let model = model.to_string();
            Arc::new(move || {
                stop_requested()
                    .lock()
                    .expect("the stop set is not contended")
                    .contains(&model)
            })
        };
        Arc::new(move || {
            local()
                || externally_stopped.load(std::sync::atomic::Ordering::SeqCst)
        })
    };

    // ── Unreachable, and when to stop asking ────────────────────────────────
    // A structural refusal does not get better on the next fixture. Counted
    // in a ROW rather than in total: one 404 among two hundred passes is a
    // blip worth recording and ignoring; three with nothing in between is a
    // fact about the whole run.
    let unreachable_state = Arc::new(Mutex::new((0usize, None::<String>)));
    let note_unreachable: Arc<dyn Fn(&str) + Send + Sync> = {
        let unreachable_state = unreachable_state.clone();
        Arc::new(move |why: &str| {
            let mut s = unreachable_state.lock().expect("one sweep, one streak");
            s.0 += 1;
            if s.1.is_none() {
                s.1 = Some(why.chars().take(300).collect());
            }
        })
    };

    let requested = opts
        .concurrency
        .unwrap_or(DEFAULT_CONCURRENCY)
        .clamp(1, MAX_CONCURRENCY);
    let valve = Arc::new(Valve::new(requested));
    let backoff_ms: Vec<u64> = opts.pressure_backoff_ms.clone().unwrap_or(PRESSURE_BACKOFF_MS.to_vec());

    let all = (deps.harnesses)().await;
    let wanted: Vec<RegisteredHarness> = match &opts.only {
        Some(only) if !only.is_empty() => all
            .iter()
            .filter(|h| only.iter().any(|id| id == h.def.id))
            .copied()
            .collect(),
        _ => all,
    };
    let metas: Vec<HarnessMeta> = wanted.iter().map(meta_of).collect();
    let unfixtured: Vec<String> = wanted
        .iter()
        .filter(|h| h.def.evals.is_empty())
        .map(|h| h.def.id.to_string())
        .collect();

    // A persisted status for THIS model is a resume point. A persisted
    // 'running' with no live sweep is a sweep a restart interrupted — also
    // resumable, and treating it as stuck instead would leave the feature
    // permanently unusable after one unlucky deploy.
    let prior = (deps.read_status)(model.to_string()).await;
    // A FINISHED RUN IS RESUMABLE WHEN YOU ARE RETRYING IT, and that is the
    // whole point of the button: the run an admin wants to re-ask five cases
    // of is one that RAN TO COMPLETION with five holes in it. Without this
    // clause `retry_failed` silently did nothing.
    let resumable = !opts.restart
        && prior.model.as_deref() == Some(model)
        && prior.state != EvalSweepState::Idle
        && (opts.retry_failed || opts.supplement || prior.state != EvalSweepState::Done);
    // A CASE THAT REACHED A CLEAN VERDICT IS EVIDENCE; anything else is a
    // hole. `retry_failed` keeps the first and re-opens the second, so the
    // sweep's ordinary resume machinery does the rest.
    //
    // EVERY FIXTURE THE REGISTRY DECLARES RIGHT NOW. A supplemental sweep is
    // exactly "the declared set minus what the ledger already answers", so it
    // needs the declared set — and computing it here also gives the prune
    // something to prune against: a verdict about a fixture that no longer
    // exists is a verdict on an assertion nobody can read, and it keeps
    // scoring the matrix.
    let declared: HashSet<String> = wanted
        .iter()
        .flat_map(|h| h.eval_names().map(|n| case_key(h.def.id, n)))
        .collect();
    let prior_cases = if resumable {
        prior.cases.clone()
    } else {
        Vec::new()
    };
    let cases: Vec<EvalCaseScore> = if opts.retry_failed {
        prior_cases
            .iter()
            .filter(|c| !worth_retrying(c))
            .cloned()
            .collect()
    } else if opts.supplement {
        prior_cases
            .iter()
            .filter(|c| declared.contains(&case_key(&c.harness, &c.case)))
            .cloned()
            .collect()
    } else {
        prior_cases
    };
    let already: HashSet<String> = cases
        .iter()
        .map(|c| case_key(&c.harness, &c.case))
        .collect();
    // Cases THIS PASS produced, as opposed to ones it inherited. Speed is
    // measured over these.
    let measured: Arc<Mutex<Vec<EvalCaseScore>>> = Arc::new(Mutex::new(Vec::new()));

    let total: usize = wanted.iter().map(|h| h.eval_names().count()).sum();
    let started_at: Option<String> = if resumable {
        prior.started_at.clone()
    } else {
        Some(iso((deps.now)()))
    };

    // SHARED, because a lane's on_case appends from inside a spawned task
    // while the loop below reads it back for the checkpoint writes.
    let cases: Arc<Mutex<Vec<EvalCaseScore>>> = Arc::new(Mutex::new(cases));

    // WAS THE GUARD PASS ACTUALLY ON? With `mode: 'off'` every `guardRate` is
    // zero, and zero-because-off must not read as zero-because-clean. Read
    // from the injected dep set when there is one (a test's set decides its
    // own answer), else from the config table the guard itself reads.
    let guarded = match &deps.harness_deps {
        Some(hd) => (hd.guard_config)()
            .await
            .is_some_and(|c| c.mode != GuardMode::Off),
        None => guard::guard_config(&state.pg).await.mode != GuardMode::Off,
    };

    // THE TWO TOOL QUESTIONS, asked ONCE — every harness below reads the same
    // answers, so a fleet listing that is slow costs one call, not
    // thirty-five.
    let own_tools = (deps.serves_own_tools)(model.to_string()).await;
    let tool_defs = (deps.accepts_tool_definitions)(model.to_string()).await;

    // The runner's dep set the whole sweep layers on — built ONCE, because
    // the real one carries reach and capability lookups the sweep should not
    // re-resolve per harness.
    let base_harness_deps: Arc<HarnessDeps> = match &deps.harness_deps {
        Some(hd) => hd.clone(),
        None => Arc::new(runner_real_deps(state)),
    };

    for h in &wanted {
        if stopped_now() {
            break;
        }
        // STOP ASKING. Two hundred and forty more cases would each buy the same
        // 404 and tell an admin nothing they did not know by case three; the
        // sweep ends with the reason instead of an hour of identical failures.
        if unreachable_state.lock().expect("one sweep, one streak").0 >= UNREACHABLE_STREAK {
            break;
        }
        let pending: Vec<&'static EvalCase> = h
            .def
            .evals
            .iter()
            .filter(|f| !already.contains(&case_key(h.def.id, f.name)))
            .collect();
        if pending.is_empty() {
            continue;
        }

        // THE SKIP, BEFORE A TOKEN IS SPENT. A harness this candidate's
        // transport is documented to refuse produces no reply on any fixture,
        // so the sweep records the absence and its reason instead of buying
        // `pending.len()` refusals and scoring them as contract failures. The
        // cases are still WRITTEN — they are the resume ledger and the
        // progress denominator, and a sweep that merely `continue`d would
        // restart into the same skip and show a progress bar that never
        // reaches its total.
        let skip = harness_skip_reason(h.def.label, h.def.tools, model, own_tools, tool_defs);
        if let Some(reason) = skip {
            for fixture in &pending {
                cases
                    .lock()
                    .expect("one sweep, one ledger")
                    .push(skipped_case(h.def.id, fixture.name, fixture.band, &reason));
            }
            write_row(
                deps,
                model,
                &started_at,
                &cases,
                total,
                EvalSweepState::Running,
                Some(h.def.id.to_string()),
                None,
            )
            .await;
            continue;
        }

        // A DRY RUN is for a tool-loop harness the PLATFORM has to drive: the
        // model cannot run its own loop here, but it can be handed
        // definitions, so the sweep supplies the loop and an isolated Talaria
        // to run it against. A fleet persona runs its own loop and needs none
        // of this.
        let dry_run = matches!(h.def.tools, Some(ToolPolicy::Own)) && !own_tools && tool_defs;

        let on_case: OnCaseFn = {
            let deps = deps.clone();
            let model = model.to_string();
            let started_at = started_at.clone();
            let archive_case = opts.archive_case.clone();
            let cases = cases.clone();
            let measured = measured.clone();
            let unreachable_state = unreachable_state.clone();
            let harness_id = h.def.id.to_string();
            Arc::new(move |score: EvalCaseScore| {
                let deps = deps.clone();
                let model = model.clone();
                let started_at = started_at.clone();
                let archive_case = archive_case.clone();
                let cases = cases.clone();
                let measured = measured.clone();
                let unreachable_state = unreachable_state.clone();
                let harness_id = harness_id.clone();
                Box::pin(async move {
                    // THE STREAK RESETS ON ANYTHING THAT REACHED THE MODEL.
                    // Only an unbroken run of refusals means the deployment
                    // cannot serve this candidate at all.
                    let resets = match &score.skipped {
                        None => true,
                        Some(reason) => !reason.contains("could not reach this model"),
                    };
                    if resets {
                        unreachable_state.lock().expect("one sweep, one streak").0 = 0;
                    }
                    cases
                        .lock()
                        .expect("one sweep, one ledger")
                        .push(score.clone());
                    measured
                        .lock()
                        .expect("one sweep, one measured set")
                        .push(score.clone());
                    // FILED AS IT LANDS, so a sweep an admin stops still keeps
                    // every transcript it paid for. Never blocks the sweep:
                    // the archive is valuable and the run is more valuable.
                    // `started_at` is the run's identity in the archive — a
                    // resumed sweep keeps the original, so its cases file
                    // under one run rather than splitting into two half-runs
                    // nobody can audit.
                    if let Some(archive) = &archive_case {
                        let _ = archive(
                            model.clone(),
                            started_at
                                .clone()
                                .unwrap_or_else(|| iso((deps.now)())),
                            score.clone(),
                        )
                        .await;
                    }
                    write_row(
                        &deps,
                        &model,
                        &started_at,
                        &cases,
                        total,
                        EvalSweepState::Running,
                        Some(harness_id.clone()),
                        None,
                    )
                    .await;
                })
            })
        };

        run_harness_cases(
            state,
            h.def,
            pending,
            model,
            deps,
            &base_harness_deps,
            timeout_ms,
            dry_run,
            &stopped_now,
            &valve,
            &backoff_ms,
            &note_unreachable,
            &on_case,
        )
        .await;
    }

    let state_out = if stopped_now() {
        EvalSweepState::Stopped
    } else {
        EvalSweepState::Done
    };
    // The run's own headline when it gave up: a routing or credential fact,
    // said ONCE — in the status row AND on the returned sweep, because the
    // archive reads one and the caller reads the other.
    let (unreachable_run, unreachable_why) =
        unreachable_state.lock().expect("one sweep, one streak").clone();
    let gave_up = (unreachable_run >= UNREACHABLE_STREAK).then(|| {
        format!(
            "the deployment could not reach this model: {}",
            unreachable_why.unwrap_or_default()
        )
    });
    write_row(
        deps,
        model,
        &started_at,
        &cases,
        total,
        state_out,
        None,
        gave_up.clone(),
    )
    .await;
    let snapshot = cases.lock().expect("one sweep, one ledger").clone();
    let measured_out = measured
        .lock()
        .expect("one sweep, one measured set")
        .clone();
    let final_status = EvalSweepStatus {
        state: state_out,
        model: Some(model.to_string()),
        started_at: started_at.clone(),
        finished_at: Some(iso((deps.now)())),
        done: snapshot.len() as i64,
        total: total as i64,
        harness: None,
        error: gave_up,
        cases: snapshot,
    };
    Ok(sweep_of(
        final_status,
        metas,
        unfixtured,
        guarded,
        Some(valve.snapshot()),
        measured_out,
    ))
}

/// One harness's pending fixtures, in declaration order, stopping between
/// cases when a stop was asked for.
#[allow(clippy::too_many_arguments)]
async fn run_harness_cases(
    state: &AppState,
    def: &'static HarnessDefinition,
    pending: Vec<&'static EvalCase>,
    model: &str,
    deps: &Arc<EvalDeps>,
    base_harness_deps: &Arc<HarnessDeps>,
    timeout_ms: u64,
    dry_run: bool,
    stopped: &StopPred,
    valve: &Arc<Valve>,
    backoff_ms: &[u64],
    note_unreachable: &Arc<dyn Fn(&str) + Send + Sync>,
    on_case: &OnCaseFn,
) {
    let worker: OnCaseWorker = {
        let state = state.clone();
        let model = model.to_string();
        let deps = deps.clone();
        let base = base_harness_deps.clone();
        let stopped = stopped.clone();
        let valve = valve.clone();
        let backoff_ms = backoff_ms.to_vec();
        let note_unreachable = note_unreachable.clone();
        let on_case = on_case.clone();
        Arc::new(move |fixture: &'static EvalCase| {
            let state = state.clone();
            let model = model.clone();
            let deps = deps.clone();
            let base = base.clone();
            let stopped = stopped.clone();
            let valve = valve.clone();
            let backoff_ms = backoff_ms.clone();
            let note_unreachable = note_unreachable.clone();
            let on_case = on_case.clone();
            Box::pin(async move {
                // RE-ASKED, NOT FAILED. Each attempt is a WHOLE fresh case — a
                // new sandbox, a new world, a clean capture — because a retry
                // that reused the previous attempt's mutated world would be
                // grading the model on a board another run had already written
                // to.
                //
                // WHICH IS ALSO WHY THE CLOCK IS OUT HERE. Each attempt starts
                // its own, so a case that was re-asked would report only the
                // surviving attempt's wall time — a case whose first two
                // requests vanished would claim to have cost four seconds when
                // it cost the sweep two minutes. `wall_ms` is documented as
                // what the case cost the sweep, so it is measured across every
                // attempt and the backoff between them.
                let opened_at = (deps.now)();
                let mut score = run_one_case(
                    &state,
                    def,
                    fixture,
                    &model,
                    &deps,
                    &base,
                    timeout_ms,
                    dry_run,
                    &stopped,
                )
                .await;
                for attempt in 0..PRESSURE_RETRIES {
                    if let Some(s) = score.as_ref()
                        && pressured(s)
                        && attempt >= retries_for(s)
                    {
                        break;
                    }
                    // Null means the case was cancelled mid-flight. Nothing is
                    // recorded: the fixture stays pending, so a resume picks it
                    // up rather than inheriting a failure that never happened.
                    let Some(s) = score.as_ref() else { break };
                    if !pressured(s) || stopped() {
                        break;
                    }
                    // THE PRESSURE VALVE, on the way past: the width comes down
                    // as well as the question being re-asked, so the retry is
                    // issued into a quieter sweep.
                    valve.narrow(s.error.as_deref().unwrap_or("the request was never answered"));
                    let ms = backoff_ms
                        .get(attempt)
                        .copied()
                        .or_else(|| backoff_ms.last().copied())
                        .unwrap_or(0);
                    backoff(ms, &stopped).await;
                    if stopped() {
                        return;
                    }
                    score = run_one_case(
                        &state,
                        def,
                        fixture,
                        &model,
                        &deps,
                        &base,
                        timeout_ms,
                        dry_run,
                        &stopped,
                    )
                    .await;
                }
                let Some(score) = score else { return };
                let mut whole = score;
                whole.started_at = iso(opened_at);
                whole.wall_ms = (deps.now)() - opened_at;
                if unreachable(&whole) {
                    note_unreachable(
                        whole
                            .error
                            .as_deref()
                            .unwrap_or("the deployment could not reach this model"),
                    );
                    (on_case)(unreachable_case(whole)).await;
                    return;
                }
                // THE OTHER HALF OF THE VALVE. A case that came back — pass or
                // fail, the grade is not this counter's business — is evidence
                // the deployment served the width it was asked at. An
                // `unreachable` case is NOT: it never left the building, so it
                // says nothing either way and returns above without voting.
                if pressured(&whole) {
                    (on_case)(rate_limited_case(whole)).await;
                    return;
                }
                valve.settled();
                (on_case)(whole).await;
            })
        })
    };
    pool(pending, valve, stopped, worker).await;
}

/// THE CHECKPOINT WRITE. After every case and skip batch — never per harness:
/// the status is both the progress bar and the resume ledger, and a sweep that
/// checkpointed per harness would re-buy a whole harness's fixtures after a
/// restart and, on the slowest harnesses, show a progress bar that does not
/// move for minutes.
#[allow(clippy::too_many_arguments)]
async fn write_row(
    deps: &EvalDeps,
    model: &str,
    started_at: &Option<String>,
    cases: &Arc<Mutex<Vec<EvalCaseScore>>>,
    total: usize,
    state: EvalSweepState,
    harness: Option<String>,
    error: Option<String>,
) {
    let snapshot = cases.lock().expect("one sweep, one ledger").clone();
    let _ = (deps.write_status)(EvalSweepStatus {
        state,
        model: Some(model.to_string()),
        started_at: started_at.clone(),
        finished_at: (state != EvalSweepState::Running).then(|| iso((deps.now)())),
        done: snapshot.len() as i64,
        total: total as i64,
        harness,
        error,
        cases: snapshot,
    })
    .await;
}

/// DID THIS CASE LEAVE A HOLE? Everything that is not a clean pass.
///
/// Deliberately generous. A skip costs nothing to re-attempt — a harness this
/// candidate's transport cannot drive skips again in microseconds, before a
/// token is spent — while a skip that was really a rate limit re-runs
/// properly. A `gap` is re-asked because the usual reason to press this button
/// is that somebody has just fixed the harness that reported it. Being
/// generous costs a few free re-skips; being narrow costs a full sweep.
pub fn worth_retrying(c: &EvalCaseScore) -> bool {
    !(c.skipped.is_none() && c.gap.is_none() && c.contract_held && c.task == TaskVerdict::Pass)
}

/// A case that never reached the model at all.
///
/// UNMEASURED, LIKE A RATE LIMIT, and for the same reason: the sweep learned
/// something about the deployment and nothing about the candidate. Recording
/// it as a contract failure is what made one routing policy look like a model
/// that fails every harness in the product.
fn unreachable_case(mut score: EvalCaseScore) -> EvalCaseScore {
    score.skipped = Some(format!(
        "the deployment could not reach this model: {}. Nothing here was measured about the model \
         itself.",
        utf16_truncate(score.error.as_deref().unwrap_or(""), 200)
    ));
    score.contract_held = false;
    score.first_pass = false;
    score.answered = false;
    score.task = TaskVerdict::Unscored;
    score.optimistic = false;
    score
}

/// A case the provider never let us ask, after every retry.
///
/// RECORDED AS UNMEASURED, NOT AS FAILED. `skipped` excludes a case from every
/// rate — which is the honest arithmetic here, because we did not learn
/// anything about the model. The alternative prints a red cell that means
/// "your provider was busy" and reads as "this model cannot hold a contract".
fn rate_limited_case(mut score: EvalCaseScore) -> EvalCaseScore {
    score.skipped = Some(if score.timed_out {
        format!(
            "the request was issued and never answered, on {} attempts, each abandoned after the \
             case budget. This case measured nothing about the model. The provider dropped the \
             call; re-run it when the deployment is healthier.",
            TIMEOUT_RETRIES + 1
        )
    } else {
        format!(
            "the provider answered with rate limits on every attempt ({} tries). This case \
             measured nothing about the model. Re-run it, narrower, when the deployment is \
             quieter.",
            PRESSURE_RETRIES + 1
        )
    });
    score.contract_held = false;
    score.first_pass = false;
    score.answered = false;
    score.task = TaskVerdict::Unscored;
    score.optimistic = false;
    score
}

/// Assemble the returned sweep. The early-return and error paths take the
/// defaults (a width of one, nothing measured) because a sweep that never ran
/// has no width to report.
fn sweep_of(
    status: EvalSweepStatus,
    metas: Vec<HarnessMeta>,
    unfixtured: Vec<String>,
    guarded: bool,
    concurrency: Option<SweepConcurrency>,
    measured: Vec<EvalCaseScore>,
) -> EvalSweep {
    EvalSweep {
        model: status.model.clone().unwrap_or_default(),
        state: status.state,
        started_at: status.started_at,
        finished_at: status.finished_at,
        done: status.done,
        total: status.total,
        error: status.error,
        harnesses: score_harnesses(&metas, &status.cases),
        cases: status.cases,
        unfixtured,
        guarded,
        concurrency: concurrency.unwrap_or(SweepConcurrency {
            requested: 1,
            ended: 1,
            low: 1,
            narrowed_because: None,
        }),
        measured,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
// Port of ui/src/server/fitness/evals.test.ts. The sweep is exercised END TO
// END THROUGH THE REAL `run_harness`, against recorded replies — the number
// this suite prints has to be the number the `harness_runs` row carries, and a
// test that stubbed the runner would be asserting about a stub. Only the edges
// are faked: the registry, the transport, and the settings row the status
// lives in. Everything else is real — the parser, the schema, the `verify`
// hook, the repair loop, the failure policies, and the final guard pass,
// which runs the actual rule registry (`run_guardrails`; this suite fakes the
// transport and the repair gate, never the rules).
//
// NOT PORTED, deliberately: `survives a fixture check that throws`. A Rust
// `CheckFn` closure cannot throw — the type has no error case — so the hazard
// that test locked (a badly written assertion taking the sweep down) cannot
// exist here. See the module header's divergence note.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityFact;
    use crate::capability_reach::{Reach, ReachVia};
    use crate::fitness::toolbox::dry_run::MAX_TURNS;
    use crate::harness::define::{
        define_harness, CheckFn, DryRunDecl, Fallback, Message, OnFailure, RenderContext,
        RoleFloor,
    };
    use crate::harness::defs::research::{CallToolFn, ToolOutput};
    use crate::harness::registry::HarnessSource;
    use crate::harness::schema::{Field, Schema};
    use crate::harness::transport::{TokenPair, ToolCall, TransportKind, TransportReply};
    use crate::harness_model::ModelSpec;

    /// THE SWEEP'S GLOBALS ARE PROCESS-WIDE — the in-flight map, the stop set,
    /// the per-model lock — and `cargo test` runs these tests on many threads
    /// in one process, where the TS file ran alone in a worker. Every test
    /// that fires a sweep takes this guard for its WHOLE body (the sweep
    /// helpers below never lock it themselves), so two sweeps never share an
    /// instant and no test's Stop can land on another's run — including the
    /// post-sweep asserts, which read the same globals.
    static ONE_SWEEP_AT_A_TIME: Mutex<()> = Mutex::new(());
    fn sole() -> std::sync::MutexGuard<'static, ()> {
        // Poison-tolerant: a test that fails an assert while holding the
        // guard poisons the mutex, and every later test would then report
        // the poison instead of its own result. The guard protects ordering,
        // never data, so there is nothing to be poisoned.
        ONE_SWEEP_AT_A_TIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ── A fake registry ─────────────────────────────────────────────────────

    fn reg(def: &'static HarnessDefinition) -> RegisteredHarness {
        RegisteredHarness {
            def,
            source: HarnessSource::Builtin,
        }
    }

    fn spec() -> ModelSpec<'static> {
        ModelSpec {
            pin: None,
            role: None,
            chain: None,
            user_id: None,
        }
    }

    fn search_floor(suppliable: bool) -> RoleFloor {
        RoleFloor {
            capabilities: vec!["search"],
            refuse_below: true,
            suppliable: if suppliable { vec!["search"] } else { vec![] },
            note: "needs live search",
        }
    }

    fn pick_schema() -> Schema {
        Schema::Object(vec![Field::required("pick", Schema::string())])
    }

    /// A JSON harness with the contract's input-relational half on `verify` —
    /// the blurb-writer shape, which is the case where a schema alone lies.
    fn picker(
        id: &'static str,
        cases: &'static [(&'static str, &'static str)],
    ) -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            id,
            "Picker",
            "Echoes back the id it was given.",
            spec(),
            Arc::new(|input: &Value, _ctx: &RenderContext| {
                let want = input.get("want").and_then(Value::as_str).unwrap_or("a");
                Ok(vec![Message::user(format!("pick {want}"))])
            }),
            Output::Json {
                schema: pick_schema(),
                preprocess: None,
                repair: None,
                verify: Some(Arc::new(
                    |value: &Value, input: &Value, _ctx: &RenderContext| {
                        let want = input.get("want").and_then(Value::as_str).unwrap_or("a");
                        let got = value.get("pick").and_then(Value::as_str).unwrap_or("");
                        Ok(if got == want {
                            None
                        } else {
                            Some(format!("the pick must be '{want}', not '{got}'"))
                        })
                    },
                )),
            },
            OnFailure::Null,
        ));
        d.requires = vec!["json"];
        d.evals = cases
            .iter()
            .map(|(name, want)| {
                let check_want = *want;
                EvalCase::new(
                    name,
                    json!({ "want": want }),
                    Arc::new(move |v: &Value, _ctx: &CheckCtx| {
                        let got = v.get("pick").and_then(Value::as_str).unwrap_or("");
                        if got == check_want {
                            CheckResult::Pass
                        } else {
                            CheckResult::Fail(format!("expected '{check_want}'"))
                        }
                    }),
                )
            })
            .collect();
        d
    }

    /// The wide defs leak their fixture names — they need `&'static str` and
    /// are each built exactly once behind a `LazyLock`, so the leak is one
    /// small allocation per def for the life of the test binary.
    fn picker_n(id: &'static str, n: usize) -> HarnessDefinition {
        let cases: Vec<(&'static str, &'static str)> = (0..n)
            .map(|i| {
                let name: &'static str = Box::leak(format!("case-{i}").into_boxed_str());
                (name, "a")
            })
            .collect();
        picker(id, Box::leak(cases.into_boxed_slice()))
    }

    static PICK_ID: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("picker", &[("echoes the id", "qwen3-14b")]));
    static PICK_A1: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("picker", &[("echoes the id", "a")]));
    static PICK_SHORT: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        let mut d = picker("picker", &[("short answers only", "a")]);
        // The fixture asserts something further than the contract — quality
        // the harness deliberately does not police.
        d.evals[0].check = Arc::new(|_v: &Value, _ctx: &CheckCtx| {
            CheckResult::Fail("the answer is not short enough".into())
        });
        d
    });
    static FALLBACK_DEF: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        let mut d = define_harness(HarnessDefinition::new(
            "fallback",
            "Fallback",
            "Has a declared safe answer.",
            spec(),
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(vec![Message::user("go")])),
            Output::Json {
                schema: pick_schema(),
                preprocess: None,
                repair: None,
                verify: None,
            },
            OnFailure::Fallback(Fallback::Json(json!({ "pick": "safe" }))),
        ));
        d.requires = vec!["json"];
        d.evals = vec![EvalCase::new(
            "holds the shape",
            json!({ "q": "x" }),
            Arc::new(|v: &Value, _ctx: &CheckCtx| {
                if v.get("pick").and_then(Value::as_str) == Some("safe") {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail("wrong".into())
                }
            }),
        )];
        d
    });
    static THROWER: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        let mut d = define_harness(HarnessDefinition::new(
            "thrower",
            "Thrower",
            "Declares onFailure: throw.",
            spec(),
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(vec![Message::user("go")])),
            Output::Json {
                schema: pick_schema(),
                preprocess: None,
                repair: None,
                verify: None,
            },
            OnFailure::Throw,
        ));
        d.requires = vec!["json"];
        d.evals = vec![EvalCase::new(
            "holds the shape",
            json!({ "q": "x" }),
            Arc::new(|_v: &Value, _ctx: &CheckCtx| CheckResult::Pass),
        )];
        d
    });
    static PICK2: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("picker", &[("one", "a"), ("two", "a")]));
    static PICK3: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("picker", &[("one", "a"), ("two", "a"), ("three", "a")]));
    static MIXED2: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("picker", &[("one", "a"), ("two", "b")]));
    static PICK3_MIX: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("picker", &[("one", "a"), ("two", "b"), ("three", "a")]));
    static FIRST2: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("first", &[("one", "a"), ("two", "a")]));
    static SECOND1: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("second", &[("three", "a")]));
    static STUCK: LazyLock<HarnessDefinition> =
        LazyLock::new(|| picker("stuck", &[("never answers", "a")]));
    static FINE: LazyLock<HarnessDefinition> = LazyLock::new(|| picker("fine", &[("answers", "a")]));
    static LOOPER1: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        let mut d = picker("looper", &[("a", "a")]);
        d.tools = Some(ToolPolicy::Own);
        d
    });
    static LOOPER2: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        let mut d = picker("looper", &[("a", "a"), ("b", "b")]);
        d.tools = Some(ToolPolicy::Own);
        d
    });
    static WIDE6: LazyLock<HarnessDefinition> = LazyLock::new(|| picker_n("wide", 6));
    static WIDE8: LazyLock<HarnessDefinition> = LazyLock::new(|| picker_n("wide", 8));
    static WIDE40: LazyLock<HarnessDefinition> = LazyLock::new(|| picker_n("wide", 40));
    static NAKED: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        let mut d = define_harness(HarnessDefinition::new(
            "naked",
            "Naked",
            "A schema and nothing else.",
            spec(),
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(vec![Message::user("go")])),
            Output::Json {
                schema: pick_schema(),
                preprocess: None,
                repair: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        d.requires = vec!["json"];
        d.evals = vec![EvalCase::new(
            "one",
            json!({ "q": "x" }),
            Arc::new(|_v: &Value, _ctx: &CheckCtx| CheckResult::Pass),
        )];
        d
    });
    static BARE: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        define_harness(HarnessDefinition::new(
            "bare",
            "Bare",
            "Declares no fixtures.",
            spec(),
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(vec![Message::user("go")])),
            Output::Text {
                clean: None,
                verify: None,
            },
            OnFailure::Null,
        ))
    });

    macro_rules! single_case_picker {
        ($($name:ident = $id:literal),* $(,)?) => {
            $( static $name: LazyLock<HarnessDefinition> =
                LazyLock::new(|| picker($id, &[("one", "a")])); )*
        };
    }
    single_case_picker! {
        H0 = "h0", H1 = "h1", H2 = "h2", H3 = "h3",
        H4 = "h4", H5 = "h5", H6 = "h6", H7 = "h7",
    }

    /// A text harness whose feature is the tool loop, dry-run against
    /// Talaria's own toolkit sandbox — `get_ticket` and `comment` are real
    /// tools over the default world, so the model's calls are genuinely
    /// dispatched and genuinely answered.
    fn dry_looper(fail_sentence: Option<&'static str>) -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            "looper",
            "Looper",
            "Works a ticket.",
            spec(),
            Arc::new(|_input: &Value, _ctx: &RenderContext| {
                Ok(vec![Message::user("work PLAT-118")])
            }),
            Output::Text {
                clean: Some(Arc::new(|raw: &str| {
                    let trimmed = raw.trim();
                    Ok((!trimmed.is_empty()).then(|| Value::String(trimmed.to_string())))
                })),
                verify: None,
            },
            OnFailure::Null,
        ));
        d.tools = Some(ToolPolicy::Own);
        d.dry_run = Some(DryRunDecl {
            tools: vec!["get_ticket", "comment"],
            max_turns: None,
            world: None,
            workspace: None,
            credentials: None,
        });
        let sentence = fail_sentence.unwrap_or("wrote first");
        d.evals = vec![EvalCase::new(
            "reads before it writes",
            json!({ "q": "go" }),
            Arc::new(move |_v: &Value, ctx: &CheckCtx| {
                if ctx.called_before("get_ticket", "comment") {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail(sentence.to_string())
                }
            }),
        )];
        d
    }
    static DRY_PASS: LazyLock<HarnessDefinition> = LazyLock::new(|| dry_looper(None));
    static DRY_FAIL: LazyLock<HarnessDefinition> =
        LazyLock::new(|| dry_looper(Some("commented on a ticket it had not read")));

    /// A text harness that declares a search floor. `suppliable` decides
    /// whether the platform may stand a tool in for the capability.
    fn searcher(suppliable: bool, check: CheckFn) -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            "searcher",
            "Searcher",
            "Answers from the live web.",
            spec(),
            Arc::new(|_input: &Value, _ctx: &RenderContext| {
                Ok(vec![Message::user("what shipped this week?")])
            }),
            Output::Text {
                clean: Some(Arc::new(|raw: &str| {
                    let trimmed = raw.trim();
                    Ok((!trimmed.is_empty()).then(|| Value::String(trimmed.to_string())))
                })),
                verify: None,
            },
            OnFailure::Null,
        ));
        d.requires = vec!["search"];
        d.floor = search_floor(suppliable);
        d.evals = vec![EvalCase::new("answers", json!({ "q": "go" }), check)];
        d
    }
    static SEARCH_TOOL_OK: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        searcher(
            true,
            Arc::new(|v: &Value, _ctx: &CheckCtx| {
                let text = v.as_str().unwrap_or("");
                if text.contains("shipped") {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail("answered from memory".into())
                }
            }),
        )
    });
    static SEARCH_GAP: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        searcher(
            true,
            Arc::new(|v: &Value, _ctx: &CheckCtx| {
                let text = v.as_str().unwrap_or("");
                if text.contains("AC-2 requires") {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail("answered with no specifics".into())
                }
            }),
        )
    });
    static SEARCH_MEM: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        searcher(
            true,
            Arc::new(|_v: &Value, _ctx: &CheckCtx| CheckResult::Pass),
        )
    });
    static SEARCH_REFUSE: LazyLock<HarnessDefinition> = LazyLock::new(|| {
        searcher(
            false,
            Arc::new(|_v: &Value, _ctx: &CheckCtx| CheckResult::Pass),
        )
    });

    // ── The fake world ──────────────────────────────────────────────────────

    /// One scripted reply. `Hang` is a promise that never settles — how a
    /// hanging harness is spelled; the clock is fired at it and nothing
    /// honors the clock, which is the case the bound exists for.
    #[derive(Clone)]
    enum Reply {
        Text(String),
        Hang,
        WithCalls(String, Vec<ToolCall>),
    }

    #[derive(Clone, Default)]
    struct World {
        /// Replies per model call, in order; the last one repeats.
        replies: Vec<Reply>,
        guard_mode: Option<GuardMode>,
        /// Does the candidate's transport run the model's own tool loop?
        /// Defaults to true (a fleet persona) — the answer that changes
        /// nothing.
        own_tools: bool,
        /// Can it be handed tool DEFINITIONS instead — the gateway's answer,
        /// and what lets the sweep supply the loop itself and dry-run.
        tool_defs: bool,
        /// Called with the 1-based call number, before the reply is handed
        /// back.
        on_call: Option<Arc<dyn Fn(usize) + Send + Sync>>,
        /// Capabilities the model is MEASURED unable to run (the floor edge).
        missing: Vec<String>,
        /// Answer the capability sheet with a measured `search: false` — the
        /// fact a probe writes. The floor consults the sheet when it refuses,
        /// so the floor-refusal test has to put the fact there rather than
        /// merely name the capability missing.
        capability_false: bool,
        /// Settle this many ms late, so overlap is observable at all.
        sleep_ms: u64,
    }

    impl World {
        fn replies(replies: Vec<Reply>) -> World {
            World {
                replies,
                ..Default::default()
            }
        }
        fn on_call(self, cb: Arc<dyn Fn(usize) + Send + Sync>) -> World {
            World {
                on_call: Some(cb),
                ..self
            }
        }
    }

    /// The edges a particular test overrides beyond the fake transport.
    #[derive(Default)]
    struct Extra {
        transport: Option<TransportFn>,
        supplier: Option<Supplier>,
        search_tool: Option<CallToolFn>,
        reach: HashMap<String, Reach>,
    }

    /// The status row, keyed the way the settings row is: ONE PER CANDIDATE.
    /// A single shared row would let one model's checkpoint resume another's
    /// sweep — the exact confusion the `does_not_resume_across_candidates`
    /// test exists to rule out.
    type StatusMap = Arc<Mutex<HashMap<String, EvalSweepStatus>>>;

    struct Bench {
        status: StatusMap,
        calls: Arc<Mutex<Vec<TransportRequest>>>,
        clock: Arc<Mutex<i64>>,
        transport: TransportFn,
        harnesses: Vec<RegisteredHarness>,
        world: World,
        deps: Arc<EvalDeps>,
    }

    impl Bench {
        fn status(&self, model: &str) -> EvalSweepStatus {
            self.status
                .lock()
                .expect("status")
                .get(model)
                .cloned()
                .unwrap_or_else(idle_status)
        }
        fn n_calls(&self) -> usize {
            self.calls.lock().expect("calls").len()
        }

        /// Same bench, rebuilt edges — the grown/shrunk registry, a wrapped
        /// transport, or a supplied tool. The status rows, the call log and
        /// the clock are SHARED, exactly like a TS `{ ...b.deps, ... }`.
        fn rebuilt(&self, tweak: impl FnOnce(&mut Rebuild)) -> Bench {
            let mut r = Rebuild {
                harnesses: self.harnesses.clone(),
                transport: self.transport.clone(),
                supplier: None,
                search_tool: None,
                reach: HashMap::new(),
            };
            tweak(&mut r);
            let extra = Extra {
                transport: Some(r.transport.clone()),
                supplier: r.supplier.clone(),
                search_tool: r.search_tool.clone(),
                reach: r.reach.clone(),
            };
            bench_from(
                r.harnesses,
                self.world.clone(),
                &extra,
                self.status.clone(),
                self.calls.clone(),
                self.clock.clone(),
            )
        }
    }

    /// What `rebuilt` may replace. The transport defaults to the bench's own
    /// (the fake with its scripted replies), so a wrapper passes through to
    /// the same world.
    struct Rebuild {
        harnesses: Vec<RegisteredHarness>,
        transport: TransportFn,
        supplier: Option<Supplier>,
        search_tool: Option<CallToolFn>,
        reach: HashMap<String, Reach>,
    }

    fn gateway_reply(text: String, tool_calls: Option<Vec<ToolCall>>) -> TransportReply {
        let tool_names = tool_calls
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|c| c.name.clone())
            .collect();
        TransportReply {
            kind: TransportKind::Gateway,
            text,
            tool_names,
            tool_calls,
            usage: Some(TokenPair {
                prompt_tokens: 40,
                completion_tokens: 10,
            }),
            contract_dropped: false,
        }
    }

    fn fake_transport(w: &World, calls: Arc<Mutex<Vec<TransportRequest>>>) -> TransportFn {
        let replies = w.replies.clone();
        let on_call = w.on_call.clone();
        let sleep_ms = w.sleep_ms;
        Arc::new(move |req| {
            let replies = replies.clone();
            let calls = calls.clone();
            let on_call = on_call.clone();
            Box::pin(async move {
                let n = {
                    let mut c = calls.lock().expect("calls");
                    c.push(req.clone());
                    c.len()
                };
                if let Some(cb) = &on_call {
                    cb(n);
                }
                if sleep_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                }
                let reply = replies.get(n - 1).cloned().unwrap_or_else(|| {
                    replies.last().cloned().unwrap_or(Reply::Text(String::new()))
                });
                match reply {
                    Reply::Hang => {
                        std::future::pending::<()>().await;
                        unreachable!("a hanging reply never settles")
                    }
                    Reply::Text(t) => Ok(gateway_reply(t, None)),
                    Reply::WithCalls(t, made) => Ok(gateway_reply(t, Some(made))),
                }
            })
        })
    }

    fn harness_deps(
        transport: TransportFn,
        w: &World,
        clock: Arc<Mutex<i64>>,
        reach: HashMap<String, Reach>,
    ) -> Arc<HarnessDeps> {
        let missing = w.missing.clone();
        let capability_false = w.capability_false;
        let guard_mode = w.guard_mode.unwrap_or(GuardMode::Observe);
        Arc::new(HarnessDeps {
            // The model is pinned by the sweep, so resolution never runs; the
            // rest are the edges that would otherwise reach a database.
            resolve_model: Arc::new(|_spec, _user| Box::pin(async { None })),
            slot_effort: Arc::new(|_model, _slot| Box::pin(async { None })),
            routing: Arc::new(|model| {
                let m = model.clone();
                Box::pin(async move { (vec!["spark".to_string()], m) })
            }),
            persona_keys: Arc::new(|_model| Box::pin(async { Vec::new() })),
            missing_capabilities: Arc::new(move |_key, _asked| {
                let m = missing.clone();
                Box::pin(async move { m })
            }),
            // No measured facts by default: the capability floor refusing a
            // weak model IS a tier-2 result, and a fake that never refused
            // would be testing the fake. The floor-refusal test opts in with
            // a measured `search: false`, because the floor reads the sheet
            // rather than the ask.
            capabilities: Arc::new(move |_key| {
                let sheet = if capability_false {
                    HashMap::from([(
                        "search".to_string(),
                        CapabilityFact {
                            value: false,
                            source: "probe".into(),
                            at: "2026-08-11T00:00:00.000Z".into(),
                            detail: None,
                            score: None,
                        },
                    )])
                } else {
                    HashMap::new()
                };
                Box::pin(async move { sheet })
            }),
            reach: Arc::new(move |_caps, _keys| {
                let r = reach.clone();
                Box::pin(async move { r })
            }),
            transport,
            guard_config: Arc::new(move || {
                Box::pin(async move {
                    Some(guard::GuardConfig {
                        mode: guard_mode,
                        checks: serde_json::Map::new(),
                        min_confidence: 0.5,
                        policed_hosts: Vec::new(),
                    })
                })
            }),
            // The REPAIR GATE only. The final guard pass runs the real rule
            // registry off the config above, so findings on the row are real
            // findings; an empty gate here is exactly the TS fake.
            guard_text: Arc::new(|_text, _input| Box::pin(async { Vec::new() })),
            record_findings: Arc::new(|_findings, _meta| Box::pin(async {})),
            record_run: Arc::new(|_row| Box::pin(async {})),
            now: Arc::new(move || {
                let mut c = clock.lock().expect("clock");
                *c += 25;
                *c
            }),
        })
    }

    fn bench_from(
        harnesses: Vec<RegisteredHarness>,
        w: World,
        extra: &Extra,
        status: StatusMap,
        calls: Arc<Mutex<Vec<TransportRequest>>>,
        clock: Arc<Mutex<i64>>,
    ) -> Bench {
        let transport = extra
            .transport
            .clone()
            .unwrap_or_else(|| fake_transport(&w, calls.clone()));
        let hd = harness_deps(transport.clone(), &w, clock.clone(), extra.reach.clone());
        let supplier: Option<SupplierFn> = match &extra.supplier {
            Some(s) => {
                let s = s.clone();
                Some(Arc::new(move |_capability| {
                    let s = s.clone();
                    Box::pin(async move { Some(s.clone()) })
                }))
            }
            None => Some(Arc::new(|_capability| Box::pin(async { None }))),
        };
        let read_status = status.clone();
        let write_status = status.clone();
        let deps = Arc::new(EvalDeps {
            harnesses: {
                let hs = harnesses.clone();
                Arc::new(move || {
                    let hs = hs.clone();
                    Box::pin(async move { hs })
                })
            },
            supplier,
            search_tool: extra.search_tool.clone(),
            harness_deps: Some(hd),
            read_status: Arc::new(move |model| {
                let st = read_status.clone();
                Box::pin(async move {
                    st.lock().expect("status").get(&model).cloned().unwrap_or_else(idle_status)
                })
            }),
            // Round-tripped through JSON the way `app_settings` stores it, so
            // a value that would not survive the settings row cannot pass.
            write_status: Arc::new(move |s: EvalSweepStatus| {
                let st = write_status.clone();
                Box::pin(async move {
                    let v = serde_json::to_value(&s).expect("serializes");
                    let back: EvalSweepStatus = serde_json::from_value(v).expect("round-trips");
                    let key = back.model.clone().unwrap_or_default();
                    st.lock().expect("status").insert(key, back);
                })
            }),
            read_all_status: Arc::new(|models| {
                Box::pin(async move {
                    models
                        .into_iter()
                        .map(|m| (m.clone(), idle_status()))
                        .collect()
                })
            }),
            price: Arc::new(|_model, p, c| {
                Box::pin(async move { Some((p + c) as f64 / 1_000_000.0) })
            }),
            serves_own_tools: {
                let v = w.own_tools;
                Arc::new(move |_model| Box::pin(async move { v }))
            },
            accepts_tool_definitions: {
                let v = w.tool_defs;
                Arc::new(move |_model| Box::pin(async move { v }))
            },
            // Real wall clock: `started_at` and the archive run id both come
            // from it, and one test places the start on a timeline.
            now: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            }),
        });
        Bench {
            status,
            calls,
            clock,
            // The same transport the sweep runs on — never a second fake —
            // so the throttled wrappers below wrap the instance whose calls
            // land in the log the asserts read.
            transport,
            harnesses,
            world: w,
            deps,
        }
    }

    /// The bench's OWN transport is the fake over the shared call log, even
    /// when `Extra` supplies a scripted one — the throttled wrappers below
    /// capture it as their pass-through, so their calls still land in the
    /// same log the asserts read.
    fn bench(harnesses: Vec<RegisteredHarness>, w: World) -> Bench {
        bench_ex(harnesses, w, &Extra::default())
    }

    fn bench_ex(harnesses: Vec<RegisteredHarness>, w: World, extra: &Extra) -> Bench {
        bench_from(
            harnesses,
            w,
            extra,
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(1_000i64)),
        )
    }

    fn test_state() -> AppState {
        let cfg = std::sync::Arc::new(
            crate::config::Config::from_parts(
                "postgres://nobody:nobody@127.0.0.1:1/none".into(),
                "redis://127.0.0.1:1/1".into(),
                "test-root".into(),
                String::new(),
                String::new(),
                "5274".into(),
            )
            .expect("test config"),
        );
        // connect_lazy: no I/O at construction. Every edge is injected, so
        // nothing here is ever hit.
        AppState::new(crate::db::pool(&cfg), cfg)
    }

    /// The caller holds ONE_SWEEP_AT_A_TIME for the whole test.
    async fn sweep(b: &Bench, model: &str) -> EvalSweep {
        sweep_with(b, model, |_| {}).await
    }

    async fn sweep_with(
        b: &Bench,
        model: &str,
        tweak: impl FnOnce(&mut EvalOptions),
    ) -> EvalSweep {
        // A HERMETIC START. A predecessor that panicked mid-sweep unwinds past
        // the engine's cleanup and leaves its model in the sweep set, and
        // every later sweep of that model would early-return the stale status
        // instead of running — reporting the predecessor's residue as this
        // test's result. The suite guard already serializes us, so the entry
        // is ours to take.
        sweeping().lock().expect("sweep set").remove(model);
        stop_requested().lock().expect("stop set").remove(model);
        let mut opts = EvalOptions {
            deps: Some(b.deps.clone()),
            ..Default::default()
        };
        tweak(&mut opts);
        run_eval_sweep(&test_state(), model, opts).await
    }

    fn obj(pick: &str) -> Reply {
        Reply::Text(json!({ "pick": pick }).to_string())
    }

    // ── Scoring ─────────────────────────────────────────────────────────────

    /// A recorded case, defaulted to a clean pass so each test states only
    /// the axis it is about.
    fn meta() -> HarnessMeta {
        HarnessMeta {
            id: "h".into(),
            label: "H".into(),
            source: "builtin".into(),
            output_kind: "json".into(),
            tools: "none".into(),
            requires: vec!["json".into()],
            verifies: true,
            repairable: true,
        }
    }

    fn score(tweak: impl FnOnce(&mut EvalCaseScore)) -> EvalCaseScore {
        let mut c = EvalCaseScore {
            harness: "h".into(),
            case: "c".into(),
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
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            estimated: false,
            timed_out: false,
            optimistic: false,
            error: None,
            prompt: None,
            raw: None,
            turns: None,
            calls: None,
            upstream: None,
        };
        tweak(&mut c);
        c
    }

    /// Twenty cases at the given first-pass and after-repair rates.
    fn population(first_pass: usize, held: usize) -> Vec<EvalCaseScore> {
        (0..20)
            .map(|i| {
                let first = i < first_pass;
                let ok = i < held;
                score(|c| {
                    c.case = format!("c{i}");
                    c.first_pass = first;
                    c.contract_held = ok;
                    c.repairs = if first { 0 } else { 1 };
                    c.task = if ok { TaskVerdict::Pass } else { TaskVerdict::Unscored };
                })
            })
            .collect()
    }

    #[test]
    fn separates_a_model_that_repairs_from_one_that_does_not() {
        // THE NUMBER THE WHOLE FEATURE EXISTS FOR. Both models are 40%
        // first-pass. One is usable behind the repair turn and one is not,
        // and before this existed nothing in Talaria could tell them apart.
        let usable = score_harness(meta(), &population(8, 19));
        let not = score_harness(meta(), &population(8, 9));

        assert!((usable.contract_rate - 0.4).abs() < 1e-9);
        assert!((not.contract_rate - 0.4).abs() < 1e-9);
        assert!((usable.repair_rate - 0.95).abs() < 1e-9);
        assert!((not.repair_rate - 0.45).abs() < 1e-9);

        // The conditional reading of the same fact: the repair turn rescued
        // essentially everything for one and almost nothing for the other.
        assert!((usable.repair_yield.unwrap() - 11.0 / 12.0).abs() < 1e-9);
        assert!((not.repair_yield.unwrap() - 1.0 / 12.0).abs() < 1e-9);
    }

    #[test]
    fn does_not_print_a_rescue_rate_for_a_repair_turn_that_cannot_happen() {
        // Thirteen of the registry's harnesses are text, and the runner sets
        // maxRepairs to 0 for every one of them. Reporting `repairYield: 0`
        // there would say "the repair turn rescued nothing" about a
        // round-trip that was never sent.
        let text = HarnessMeta {
            output_kind: "text".into(),
            repairable: false,
            ..meta()
        };
        let s = score_harness(text, &population(8, 8));
        assert!(s.repair_yield.is_none());
        assert!((s.repair_rate - 0.4).abs() < 1e-9);
        assert!((s.contract_rate - 0.4).abs() < 1e-9);

        // Zero on a harness that CAN repair is a real and much worse fact,
        // and the two must not print the same.
        assert_eq!(score_harness(meta(), &population(8, 8)).repair_yield, Some(0.0));
    }

    #[test]
    fn is_cumulative_so_repair_rate_never_reads_below_contract_rate() {
        let s = score_harness(meta(), &population(20, 20));
        assert_eq!(s.contract_rate, 1.0);
        assert_eq!(s.repair_rate, 1.0);
        assert!(s.repair_yield.is_none());
    }

    #[test]
    fn leaves_task_score_null_when_nothing_was_task_scorable() {
        let s = score_harness(
            meta(),
            &[score(|c| {
                c.contract_held = false;
                c.first_pass = false;
                c.task = TaskVerdict::Unscored;
            })],
        );
        assert!(s.task_score.is_none());
        assert_eq!(s.repair_rate, 0.0);
    }

    #[test]
    fn averages_guard_findings_per_run_and_counts_timeouts_out_of_the_sample() {
        let s = score_harness(
            meta(),
            &[
                score(|c| {
                    c.case = "a".into();
                    c.findings = 2;
                    c.latency_ms = 10;
                }),
                score(|c| {
                    c.case = "b".into();
                    c.findings = 0;
                    c.latency_ms = 90;
                }),
                score(|c| {
                    c.case = "c".into();
                    c.timed_out = true;
                    c.latency_ms = 0;
                    c.contract_held = false;
                    c.first_pass = false;
                    c.task = TaskVerdict::Unscored;
                }),
            ],
        );
        // Two findings over the two cases that produced a reply. A timed-out
        // case never reached the guard pass, so leaving it in the denominator
        // would dilute the rate by the share of cases that ran out of clock.
        assert_eq!(s.guard_rate, 1.0);
        assert_eq!(s.timeouts, 1);
        assert_eq!(s.scored, 2);
        // THE RATES DIVIDE BY `scored`, NOT BY `cases`. A timeout observed
        // nothing about the model's contract — the clock ran out, which is a
        // fact about our budget and the provider's latency.
        assert_eq!(s.cases, 3);
        assert_eq!(s.contract_rate, 1.0);
        assert_eq!(s.repair_rate, 1.0);
        // Nearest-rank over the two cases that actually measured something.
        assert_eq!(s.latency_p50, 10);
        assert_eq!(s.latency_p95, 90);
    }

    // ── The sweep ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn scores_the_contract_from_the_row_the_runner_writes() {
        let _sole = sole();
        // The reply parses against the schema and FAILS `verify` — the exact
        // shape of the blurb-writer bug, where the schema said yes and the
        // caller threw the value away. The runner records schema_valid false
        // for it; so does this suite, because it reads that row rather than
        // re-deciding.
        let b = bench(
            vec![reg(&*PICK_ID)],
            World::replies(vec![obj("Qwen3 14B"), obj("Qwen3 14B")]),
        );

        let sweep = sweep(&b, "candidate").await;
        let one = &sweep.cases[0];

        assert!(!one.contract_held);
        assert!(!one.first_pass);
        // The runner's own repair turn fired and the model repeated itself.
        assert_eq!(one.repairs, 1);
        assert!(one.answered);
        // No model value survived, so the fixture graded nothing — a contract
        // failure must not be charged twice.
        assert_eq!(one.task, TaskVerdict::Unscored);
        assert!(
            one.error
                .as_deref()
                .unwrap_or("")
                .contains("the pick must be 'qwen3-14b'")
        );
        // The drill-down keeps the prompt and the reply for a case that failed.
        assert!(one.prompt.as_deref().unwrap_or("").contains("pick qwen3-14b"));
        assert!(one.raw.as_deref().unwrap_or("").contains("Qwen3 14B"));

        assert_eq!(sweep.harnesses[0].contract_rate, 0.0);
        assert_eq!(sweep.harnesses[0].repair_rate, 0.0);
        assert_eq!(sweep.state, EvalSweepState::Done);
    }

    #[tokio::test]
    async fn counts_a_reply_only_the_repair_turn_fixed_as_repaired() {
        let _sole = sole();
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![obj("A"), obj("a")]));

        let sweep = sweep(&b, "candidate").await;
        let one = &sweep.cases[0];

        assert!(one.contract_held);
        assert!(!one.first_pass);
        assert_eq!(one.repairs, 1);
        assert_eq!(one.task, TaskVerdict::Pass);
        assert_eq!(sweep.harnesses[0].contract_rate, 0.0);
        assert_eq!(sweep.harnesses[0].repair_rate, 1.0);
        assert_eq!(sweep.harnesses[0].repair_yield, Some(1.0));
    }

    #[tokio::test]
    async fn flags_a_value_the_contract_accepted_and_the_fixture_rejected() {
        let _sole = sole();
        // The contract holds (the id came back verbatim) and the fixture
        // asserts something further — quality the harness deliberately does
        // not police. That is `optimistic`: expected here, a bug where the
        // assertion is one the caller depends on.
        let b = bench(vec![reg(&*PICK_SHORT)], World::replies(vec![obj("a")]));

        let sweep = sweep(&b, "candidate").await;
        assert!(sweep.cases[0].contract_held);
        assert_eq!(sweep.cases[0].task, TaskVerdict::Fail);
        assert!(sweep.cases[0].optimistic);
        assert_eq!(sweep.harnesses[0].optimistic, 1);
        assert_eq!(sweep.harnesses[0].task_score, Some(0.0));
        assert_eq!(sweep.harnesses[0].contract_rate, 1.0);
    }

    #[tokio::test]
    async fn does_not_award_task_points_for_a_declared_fallback() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*FALLBACK_DEF)],
            World::replies(vec![
                Reply::Text("not json at all".into()),
                Reply::Text("still not json".into()),
            ]),
        );

        let sweep = sweep(&b, "candidate").await;
        // The runner hands the fallback back with schema_valid false. Grading
        // it would score the harness author's constant as a model win.
        assert!(!sweep.cases[0].contract_held);
        assert_eq!(sweep.cases[0].task, TaskVerdict::Unscored);
        assert!(sweep.harnesses[0].task_score.is_none());
    }

    #[tokio::test]
    async fn records_the_guard_findings_the_run_row_counted() {
        let _sole = sole();
        let h = &*PICK_A1;
        // `secret_leak` over a live-looking key in the reply. The final guard
        // pass runs the real rule registry off the config — this suite fakes
        // the transport and the repair gate, never the rules.
        let leak = json!({
            "pick": "a",
            "note": "use sk-live-9f4c2a7b1e6d80541122334455667788"
        })
        .to_string();
        let b = bench(vec![reg(h)], World::replies(vec![Reply::Text(leak)]));

        let leaky = sweep(&b, "candidate").await;
        assert!(leaky.guarded);
        assert!(leaky.cases[0].findings > 0);
        assert!(leaky.harnesses[0].guard_rate > 0.0);

        let off = bench(
            vec![reg(h)],
            World {
                replies: vec![obj("a")],
                guard_mode: Some(GuardMode::Off),
                ..Default::default()
            },
        );
        // Zero findings, and the sweep says WHY — zero-because-off must not
        // read as zero-because-clean.
        let quiet = sweep(&off, "candidate").await;
        assert!(!quiet.guarded);
        assert_eq!(quiet.cases[0].findings, 0);
    }

    #[tokio::test]
    async fn bounds_a_hanging_harness_and_keeps_going() {
        let _sole = sole();
        // The first call never settles and never rejects: a persona container
        // that accepted the connection and went away. The clock is fired at
        // it and this transport, like several real ones, does not honor it.
        let b = bench(
            vec![reg(&*STUCK), reg(&*FINE)],
            World::replies(vec![Reply::Hang, obj("a")]),
        );

        // `caseTimeoutMs` is the PER-TURN allowance, not the whole case: a
        // case is not one model call (a dry run takes up to `MAX_TURNS`, a
        // JSON harness can add a repair), and a flat budget applied to a
        // multi-call case timed out the harnesses that call the most and
        // charged it to the model. These fixtures are JSON with one repair
        // turn, so 30ms per turn is 60ms a case.
        let sweep = sweep_with(&b, "candidate", |o| {
            o.case_timeout_ms = Some(30);
            o.pressure_backoff_ms = Some(vec![1]);
        })
        .await;

        // THREE CALLS, NOT TWO: the stuck case's first request is lost, so it
        // is asked ONCE more (`TIMEOUT_RETRIES`) before the sweep gives up on
        // it — here the second attempt answers, which is the whole argument
        // for the retry — plus the fine harness's one call.
        assert_eq!(b.n_calls(), 3);
        let stuck_case = sweep.cases.iter().find(|c| c.harness == "stuck").unwrap();
        assert!(!stuck_case.timed_out);
        assert!(stuck_case.contract_held);
        // The sweep did not strand: the next harness ran and scored.
        assert!(sweep.cases.iter().find(|c| c.harness == "fine").unwrap().contract_held);
        assert_eq!(sweep.state, EvalSweepState::Done);
        assert_eq!(sweep.done, 2);
    }

    #[tokio::test]
    async fn does_not_let_a_throwing_harness_end_the_sweep() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*THROWER), reg(&*FINE)],
            World::replies(vec![
                Reply::Text("nope".into()),
                Reply::Text("nope".into()),
                obj("a"),
            ]),
        );

        let sweep = sweep(&b, "candidate").await;
        let failed = sweep.cases.iter().find(|c| c.harness == "thrower").unwrap();
        assert!(!failed.contract_held);
        // The row is written before the throw, so the failure stays visible
        // with its repair count and its sentence intact.
        assert_eq!(failed.repairs, 1);
        assert!(failed.error.as_deref().unwrap_or("").contains("thrower"));
        assert!(sweep.cases.iter().find(|c| c.harness == "fine").unwrap().contract_held);
        assert_eq!(sweep.state, EvalSweepState::Done);
    }

    #[tokio::test]
    async fn leaves_consistent_resumable_state_when_an_admin_stops_it() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*FIRST2), reg(&*SECOND1)],
            World::replies(vec![obj("a")]).on_call(Arc::new(|call| {
                // Stop after the first case has been answered, so this
                // exercises the BOUNDARY path. The mid-case path is the test
                // below.
                if call == 1 {
                    stop_eval_sweep(Some("candidate"));
                }
            })),
        );

        // WIDTH 1, because this test is about stop-and-resume semantics
        // rather than about concurrency: at the default width several cases
        // are already in flight when the stop lands, so "exactly one case was
        // recorded" would be asserting the pool's timing instead of the
        // ledger's consistency.
        let stopped = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;

        assert_eq!(stopped.state, EvalSweepState::Stopped);
        // CONSISTENT: the ledger, the counter and the persisted row all
        // agree, and nothing was scored for a case that never ran.
        assert_eq!(stopped.cases.len(), 1);
        assert_eq!(stopped.done, 1);
        assert_eq!(stopped.total, 3);
        let status = b.status("candidate");
        assert_eq!(status.state, EvalSweepState::Stopped);
        assert_eq!(status.done, 1);
        assert_eq!(
            status
                .cases
                .iter()
                .map(|c| case_key(&c.harness, &c.case))
                .collect::<Vec<_>>(),
            vec!["first::one"]
        );
        assert!(status.finished_at.is_some());
        assert_eq!(b.n_calls(), 1);

        // RESUMABLE: the same candidate picks up where it left off and does
        // not re-buy the case it already paid for.
        let resumed = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;
        assert_eq!(resumed.state, EvalSweepState::Done);
        assert_eq!(resumed.done, 3);
        assert_eq!(
            resumed
                .cases
                .iter()
                .map(|c| case_key(&c.harness, &c.case))
                .collect::<Vec<_>>(),
            vec!["first::one", "first::two", "second::three"]
        );
        assert_eq!(b.n_calls(), 3);
    }

    #[tokio::test]
    async fn does_not_resume_across_candidates() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*PICK2)],
            World::replies(vec![obj("a")]).on_call(Arc::new(|call| {
                if call == 1 {
                    stop_eval_sweep(Some("candidate-a"));
                }
            })),
        );
        // Width 1 for the same reason as above: this is about whose ledger
        // it is, not about how many cases were in flight when the stop
        // landed.
        sweep_with(&b, "candidate-a", |o| o.concurrency = Some(1)).await;
        assert_eq!(b.status("candidate-a").cases.len(), 1);

        // A different model discards the ledger: a matrix cell assembled from
        // two models is a number with no referent.
        let other = sweep(&b, "candidate-b").await;
        assert_eq!(other.model, "candidate-b");
        assert_eq!(other.done, 2);
        assert!(other.cases.iter().all(|c| c.contract_held));
    }

    #[tokio::test]
    async fn records_a_tool_loop_harness_as_skipped_only_when_nothing_drives_it() {
        let _sole = sole();
        // THE DEFECT THIS LOCKS. Harnesses whose feature IS the tool loop
        // declare `tools: 'own'`. On an org-gateway model the transport
        // refuses them in about four milliseconds, before a token is spent —
        // and the sweep used to record that refusal as `contractHeld:
        // false`, so the matrix printed "0% first pass" for a model nothing
        // had asked a question.
        //
        // A model that can be handed DEFINITIONS is dry-run instead (below);
        // this is the remaining case, where neither path exists and a skip is
        // the honest answer.
        let b = bench(
            vec![reg(&*LOOPER2), reg(&*PICK_A1)],
            World {
                replies: vec![obj("a")],
                own_tools: false,
                tool_defs: false,
                ..Default::default()
            },
        );

        let sweep = sweep(&b, "gw/model").await;

        // ONE call, for the one harness that could run. The skipped fixtures
        // cost nothing, which is half the point.
        assert_eq!(b.n_calls(), 1);

        let skipped: Vec<&EvalCaseScore> =
            sweep.cases.iter().filter(|c| c.skipped.is_some()).collect();
        assert_eq!(skipped.len(), 2);
        assert!(
            skipped[0]
                .skipped
                .as_deref()
                .unwrap_or("")
                .contains("neither run its own nor be handed tool definitions")
        );
        // No transcript on a case that never ran: there is nothing to drill
        // into.
        assert!(skipped[0].prompt.is_none());
        assert!(skipped[0].error.is_none());

        let looped = sweep.harnesses.iter().find(|h| h.meta.id == "looper").unwrap();
        // `cases` is the RUN denominator. Zero of them ran, so every rate is
        // the n===0 zero and `skipped` carries the count — a consumer reading
        // `cases` sees "no evidence", which is the truth.
        assert_eq!(looped.cases, 0);
        assert_eq!(looped.skipped, 2);
        assert!(
            looped
                .skip_reason
                .as_deref()
                .unwrap_or("")
                .contains("nothing here is a measurement of it")
        );

        // The harness that CAN run is untouched by any of this.
        let plain = sweep.harnesses.iter().find(|h| h.meta.id == "picker").unwrap();
        assert_eq!(plain.contract_rate, 1.0);
        // Progress still reaches its total, so the bar completes and a resume
        // does not re-enter the same skip.
        assert_eq!(sweep.done, sweep.total);
    }

    #[tokio::test]
    async fn runs_a_tool_loop_harness_normally_for_a_fleet_persona() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*LOOPER1)],
            World {
                replies: vec![obj("a")],
                own_tools: true,
                ..Default::default()
            },
        );

        let sweep = sweep(&b, "engineer-engineering").await;

        assert_eq!(b.n_calls(), 1);
        assert!(sweep.cases[0].skipped.is_none());
        assert_eq!(sweep.harnesses[0].cases, 1);
        assert_eq!(sweep.harnesses[0].skipped, 0);
    }

    #[tokio::test]
    async fn records_the_whole_tool_conversation_on_a_dry_run() {
        let _sole = sole();
        // THE DRILL-DOWN'S RAW MATERIAL. A behavioural fixture's verdict is
        // one sentence, and that sentence is either about the model or about
        // our harness. Nothing on the page could tell those apart until the
        // turns and the calls were archived beside it.
        let b = bench(
            vec![reg(&*DRY_PASS)],
            World {
                replies: vec![
                    Reply::WithCalls(
                        String::new(),
                        vec![
                            ToolCall {
                                name: "get_ticket".into(),
                                id: None,
                                args: json!({ "taskId": "PLAT-118" }).to_string(),
                            },
                            ToolCall {
                                name: "comment".into(),
                                id: None,
                                args: json!({ "taskId": "PLAT-118", "content": "On it." })
                                    .to_string(),
                            },
                        ],
                    ),
                    Reply::Text("Read it and acknowledged.".into()),
                ],
                own_tools: false,
                tool_defs: true,
                ..Default::default()
            },
        );

        let sweep = sweep(&b, "gw/model").await;
        let c = &sweep.cases[0];

        assert_eq!(c.task, TaskVerdict::Pass);
        // THE CALLS SURVIVE A PASS. Comparing two models on one fixture IS
        // comparing these two lists, so keeping them only on failure would
        // hide the comparison worth making.
        let calls = c.calls.as_ref().expect("calls survive a pass");
        assert_eq!(
            calls.iter().map(|x| x.tool.as_str()).collect::<Vec<_>>(),
            vec!["get_ticket", "comment"]
        );
        let second: Value = serde_json::from_str(&calls[1].args).expect("the args are JSON");
        assert_eq!(second["content"], json!("On it."));
        // Results are dropped on a clean case — they are the whole weight,
        // and there is nothing to explain. The transcript goes for the same
        // reason.
        assert!(calls.iter().all(|x| x.result.is_none()));
        assert!(c.turns.is_none());
    }

    #[tokio::test]
    async fn keeps_the_turns_and_tool_results_when_a_dry_run_fails_its_check() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*DRY_FAIL)],
            World {
                replies: vec![
                    Reply::WithCalls(
                        String::new(),
                        vec![ToolCall {
                            name: "comment".into(),
                            id: None,
                            args: json!({ "taskId": "PLAT-118", "content": "On it." }).to_string(),
                        }],
                    ),
                    Reply::Text("Commented.".into()),
                ],
                own_tools: false,
                tool_defs: true,
                ..Default::default()
            },
        );

        let sweep = sweep(&b, "gw/model").await;
        let c = &sweep.cases[0];

        assert_eq!(
            c.task_error.as_deref(),
            Some("commented on a ticket it had not read")
        );
        // The tool ANSWERED, and on a failure that answer is often the
        // explanation.
        let calls = c.calls.as_ref().expect("calls are kept on a failure");
        assert!(calls[0].result.is_some() || calls[0].error.is_some());
        // The transcript: a user turn, an assistant turn carrying the call,
        // and a tool turn carrying the result.
        let turns = c.turns.as_ref().expect("turns are kept on a failure");
        let roles: Vec<&str> = turns.iter().map(|t| t.role.as_str()).collect();
        assert_eq!(roles.first(), Some(&"user"));
        assert!(roles.contains(&"assistant"));
        assert!(roles.contains(&"tool"));
        let assistant = turns.iter().find(|t| t.role == "assistant").unwrap();
        assert_eq!(
            assistant.tool_calls.as_deref().unwrap_or(&[]),
            &["comment".to_string()]
        );
    }

    #[tokio::test]
    async fn stops_a_case_already_in_flight_and_records_nothing_for_it() {
        let _sole = sole();
        // WHY THE BUTTON READ AS BROKEN. Stop used to be honored only between
        // cases, and a dry-run case is budgeted `PER_TURN_TIMEOUT_MS ×
        // turnsPerCase` — minutes. The request landed immediately and the
        // sweep politely finished a case nobody wanted.
        let b = bench(
            vec![reg(&*PICK2)],
            World::replies(vec![Reply::Hang]).on_call(Arc::new(|_call| {
                stop_eval_sweep(Some("candidate"));
            })),
        );

        let at = std::time::Instant::now();
        let stopped = sweep_with(&b, "candidate", |o| {
            o.case_timeout_ms = Some(600_000);
            o.concurrency = Some(1);
        })
        .await;
        let took = at.elapsed();

        assert_eq!(stopped.state, EvalSweepState::Stopped);
        // Under the case budget by three orders of magnitude. Before the fix
        // this would have sat here for the full ten minutes.
        assert!(took.as_secs() < 5, "stop must outrun the case budget");
        // CANCELLED IS NOT FAILED. Nothing is written for the case that was
        // killed: the persisted status is the resume ledger, so recording it
        // would mark the fixture done, skip it forever on resume, and leave
        // the model carrying a failure it was never given a chance at.
        assert!(stopped.cases.is_empty());
        assert_eq!(stopped.done, 0);
        assert!(b.status("candidate").cases.is_empty());
        // It really was mid-flight: one call went out and no second one
        // followed.
        assert_eq!(b.n_calls(), 1);
    }

    #[tokio::test]
    async fn honors_a_stop_asked_for_by_another_instance_mid_case() {
        let _sole = sole();
        // `stopRequested` is in-process and empty in any worker that did not
        // start the run, so a cross-process Stop only ever arrives through
        // `shouldStop`. That used to be read once per HARNESS — eleven
        // work-session fixtures at up to seven minutes each before it was
        // noticed.
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![Reply::Hang]));

        // FALSE AT THE FIRST ASK, TRUE AFTER — otherwise the sweep would
        // break before it ever started a case and this test would pass
        // without exercising the path it is about. The bool is computed
        // BEFORE the async block because a MutexGuard cannot be captured
        // into a 'static future.
        let asks = Arc::new(Mutex::new(0usize));
        let asks_reader = asks.clone();
        let at = std::time::Instant::now();
        let stopped = sweep_with(&b, "candidate", |o| {
            o.case_timeout_ms = Some(600_000);
            o.should_stop = Some(Arc::new(move |_model| {
                let stop = {
                    let mut a = asks_reader.lock().expect("asks");
                    *a += 1;
                    *a > 1
                };
                Box::pin(async move { stop })
            }));
        })
        .await;

        // It really did start the case before the stop arrived.
        assert_eq!(b.n_calls(), 1);
        assert!(*asks.lock().expect("asks") > 1);
        assert_eq!(stopped.state, EvalSweepState::Stopped);
        assert!(at.elapsed().as_secs() < 5, "stop must outrun the case budget");
        assert!(stopped.cases.is_empty());
    }

    #[tokio::test]
    async fn says_what_a_timed_out_case_was_waiting_on() {
        let _sole = sole();
        // THREE ROUNDS OF "still getting timeouts" WENT PAST ON ONE SENTENCE.
        // "The case did not finish inside 60000ms" is the least useful true
        // statement available: it cannot tell a slow model from a request
        // that never came back from a case that spent its budget on retries
        // from time that never reached the provider at all.
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![Reply::Hang]));

        let sweep = sweep_with(&b, "candidate", |o| {
            o.case_timeout_ms = Some(60);
            o.pressure_backoff_ms = Some(vec![1]);
        })
        .await;
        let c = &sweep.cases[0];

        assert!(c.timed_out);
        // The call went out and never came back — which is the whole
        // diagnosis, and it survives onto the recorded case even after the
        // retry has been spent.
        let upstream = c.upstream.as_ref().expect("upstream attempts recorded");
        assert!(!upstream.is_empty());
        assert!(upstream.iter().all(|u| !u.settled));
        let detail = c.error.as_deref().unwrap_or("");
        assert!(detail.contains("upstream call"), "the detail says what waited: {detail}");
        assert!(detail.contains("still had no reply"), "the detail says how long: {detail}");
        // A request that never came back is UNMEASURED, not a slow model: the
        // skip carries that sentence and `error` keeps the diagnostic, so the
        // cell neither scores the model nor hides what happened.
        assert!(
            c.skipped.as_deref().unwrap_or("").contains("never answered"),
            "a lost request is recorded unmeasured: {:?}",
            c.skipped
        );
    }

    #[tokio::test]
    async fn publishes_the_case_it_is_on_and_clears_it_when_the_case_ends() {
        let _sole = sole();
        // WHAT A WEDGED SWEEP LOOKS LIKE OTHERWISE: a still image. The
        // completed-case feed cannot show the case that is not completing,
        // which is always the one worth looking at.
        let seen: Arc<Mutex<Vec<(String, String, i64, usize)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let b = bench(
            vec![reg(&*PICK_A1)],
            World::replies(vec![obj("a")]).on_call(Arc::new(move |_call| {
                for f in in_flight_for("candidate") {
                    seen_cb.lock().expect("seen").push((
                        f.harness.clone(),
                        f.case.clone(),
                        f.turn,
                        f.turns.len(),
                    ));
                }
            })),
        );

        sweep(&b, "candidate").await;

        let seen = seen.lock().expect("seen").clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "picker");
        assert_eq!(seen[0].1, "echoes the id");
        assert!(seen[0].2 >= 1, "the turn counter is live");
        assert!(seen[0].3 > 0, "the turns are populated before the reply lands");

        // CLEARED. A "running now" that outlives its sweep makes a finished
        // run look wedged, which is the confusion this panel exists to
        // remove. (Still holding the sweep guard, so no other test's run can
        // be publishing under this model.)
        assert!(in_flight_for("candidate").is_empty());
    }

    #[tokio::test]
    async fn clears_the_in_flight_case_when_a_run_is_stopped_mid_case() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*PICK_A1)],
            World::replies(vec![Reply::Hang]).on_call(Arc::new(|_call| {
                stop_eval_sweep(Some("candidate"));
            })),
        );
        sweep_with(&b, "candidate", |o| o.case_timeout_ms = Some(600_000)).await;
        assert!(in_flight_for("candidate").is_empty());
    }

    #[tokio::test]
    async fn runs_a_harnesses_fixtures_in_parallel_bounded_by_the_width() {
        let _sole = sole();
        // A 247-fixture sweep one at a time is most of an hour. What the old
        // sequential rule was protecting is preserved elsewhere — the width is
        // recorded so latency stays interpretable, and the pressure valve
        // below handles the deployment that cannot take it.
        let b = bench(vec![reg(&*WIDE8)], World::replies(vec![obj("a")]));
        // Count overlap ACROSS the await, not at call arrival: the transport
        // holds each call open a little so concurrent lanes are genuinely
        // observable, and the high-water mark is taken while they are open.
        let live = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let base = b.transport.clone();
        let live_w = live.clone();
        let peak_w = peak.clone();
        let slow = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let live = live_w.clone();
                let peak = peak_w.clone();
                Box::pin(async move {
                    let n = live.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let reply = base(req).await;
                    live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    reply
                })
            });
        });

        let sweep = sweep_with(&slow, "candidate", |o| o.concurrency = Some(3)).await;

        assert_eq!(sweep.done, 8);
        assert!(sweep.cases.iter().all(|c| c.contract_held));
        // Bounded: never more than the width, and more than one.
        let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
        assert!(peak > 1, "the sweep must actually overlap (peak {peak})");
        assert!(peak <= 3, "the width must bound it (peak {peak})");
        assert_eq!(sweep.concurrency.requested, 3);
    }

    #[tokio::test]
    async fn records_every_case_exactly_once_whatever_order_they_finish_in() {
        let _sole = sole();
        // Completion order is not submission order under a pool, and the
        // resume ledger is a SET of case keys — so the thing to prove is that
        // the ledger and the counter still agree when the order is
        // scrambled.
        let b = bench(vec![reg(&*WIDE6)], World::replies(vec![obj("a")]));

        let sweep = sweep_with(&b, "candidate", |o| o.concurrency = Some(4)).await;

        let keys = sweep
            .cases
            .iter()
            .map(|c| case_key(&c.harness, &c.case))
            .collect::<Vec<_>>();
        let unique: HashSet<&String> = keys.iter().collect();
        assert_eq!(unique.len(), 6);
        assert_eq!(sweep.done, 6);
        assert_eq!(b.status("candidate").done, 6);
    }

    // The valve. Every throttled transport below wraps the bench's own, the
    // way the TS tests spread `harnessDeps` over the original — the wrapper
    // decides which calls the provider refuses and everything else passes
    // through unchanged.

    #[tokio::test]
    async fn narrows_itself_when_the_provider_answers_with_rate_limits() {
        let _sole = sole();
        // THE HALF THE SEQUENTIAL RULE EXISTED FOR. A self-hosted model
        // behind one GPU answers a parallel sweep with 429s, and scoring
        // those as contract failures records a fact about the hardware as a
        // fact about the model. So the sweep backs off instead, and the
        // report carries the reason.
        let seq = Arc::new(Mutex::new(0usize));
        let seq_wrap = seq.clone();
        let b = bench(vec![reg(&*WIDE8)], World::replies(vec![obj("a")]));
        let base = b.transport.clone();
        let throttled = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let seq = seq_wrap.clone();
                Box::pin(async move {
                    let n = {
                        let mut s = seq.lock().expect("seq");
                        *s += 1;
                        *s
                    };
                    // The first few calls come back rate-limited; the rest
                    // are fine.
                    if n <= 2 {
                        return Err("gateway completion 429: too many requests".into());
                    }
                    base(req).await
                })
            });
        });

        let sweep = sweep_with(&throttled, "candidate", |o| {
            o.concurrency = Some(8);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;

        assert_eq!(sweep.concurrency.requested, 8);
        // `low`, not `ended`: the valve reopens now, so a short run that
        // recovers can finish back at the width it started from. What the
        // narrowing left behind is the floor it reached.
        assert!(sweep.concurrency.low < 8);
        assert!(
            sweep
                .concurrency
                .narrowed_because
                .as_deref()
                .unwrap_or("")
                .contains("429")
        );
    }

    #[tokio::test]
    async fn narrows_inside_the_harness_that_hit_the_pressure() {
        let _sole = sole();
        // THE BUG. `pool` used to read the width ONCE, to size its lane
        // array, and then ran those lanes to exhaustion — so narrowing did
        // nothing until the next harness built a new pool. A single-harness
        // sweep (or the last harness of any sweep) therefore ran at full
        // width no matter how hard the provider pushed back.
        let live = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let seq = Arc::new(Mutex::new(0usize));
        let samples: Arc<Mutex<Vec<(usize, isize)>>> = Arc::new(Mutex::new(Vec::new()));
        let b = bench(vec![reg(&*WIDE40)], World::replies(vec![obj("a")]));
        let base = b.transport.clone();
        let live_wrap = live.clone();
        let seq_wrap = seq.clone();
        let samples_wrap = samples.clone();
        let throttled = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let live = live_wrap.clone();
                let seq = seq_wrap.clone();
                let samples = samples_wrap.clone();
                Box::pin(async move {
                    let n = {
                        let mut s = seq.lock().expect("seq");
                        *s += 1;
                        *s
                    };
                    // The very first call is the only pressure. One lost
                    // minute, then a deployment that behaves perfectly for
                    // the remaining thirty-nine.
                    if n == 1 {
                        return Err("gateway completion 429: too many requests".into());
                    }
                    let now = live.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    samples.lock().expect("samples").push((n, now));
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let reply = base(req).await;
                    live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    reply
                })
            });
        });

        let sweep = sweep_with(&throttled, "candidate", |o| {
            o.concurrency = Some(8);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;

        assert!(
            sweep
                .concurrency
                .narrowed_because
                .as_deref()
                .unwrap_or("")
                .contains("429")
        );
        // The eight lanes launched together, so the opening burst
        // legitimately overlaps at full width — the claim is about what
        // happens AFTER the valve has been told. From call twelve on, four
        // lanes are parked and stay parked until the recovery streak earns
        // them back one at a time.
        let samples = samples.lock().expect("samples").clone();
        let late: Vec<_> = samples.iter().filter(|(n, _)| *n >= 12).collect();
        assert!(!late.is_empty());
        assert!(late.iter().map(|(_, l)| *l).max().unwrap_or(0) <= 8);
        let narrowed_window: Vec<isize> = samples
            .iter()
            .filter(|(n, _)| (12..=20).contains(n))
            .map(|(_, l)| *l)
            .collect();
        assert!(!narrowed_window.is_empty());
        assert!(narrowed_window.iter().min().unwrap_or(&8) < &8);
    }

    #[tokio::test]
    async fn reopens_the_valve_after_a_clean_stretch() {
        let _sole = sole();
        // WHAT THIS COST IN PRODUCTION. A 247-case sweep asked for width 4,
        // met one lost request in its first minute, and ran the remaining two
        // hundred and forty cases strictly sequentially. The archive said so
        // plainly — `requested: 4, ended: 1` — and nobody was told.
        let seq = Arc::new(Mutex::new(0usize));
        let b = bench(vec![reg(&*WIDE40)], World::replies(vec![obj("a")]));
        let base = b.transport.clone();
        let seq_wrap = seq.clone();
        let flaky = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let seq = seq_wrap.clone();
                Box::pin(async move {
                    let n = {
                        let mut s = seq.lock().expect("seq");
                        *s += 1;
                        *s
                    };
                    if n == 1 {
                        return Err("gateway completion 429: too many requests".into());
                    }
                    base(req).await
                })
            });
        });

        let sweep = sweep_with(&flaky, "candidate", |o| {
            o.concurrency = Some(4);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;

        assert_eq!(sweep.concurrency.requested, 4);
        // It DID narrow — the reason is recorded and the floor is below what
        // was asked for — and it climbed all the way back before the run
        // ended.
        assert!(
            sweep
                .concurrency
                .narrowed_because
                .as_deref()
                .unwrap_or("")
                .contains("429")
        );
        assert!(sweep.concurrency.low < 4);
        assert_eq!(sweep.concurrency.ended, 4);
    }

    #[tokio::test]
    async fn does_not_reopen_while_the_provider_is_still_pushing_back() {
        let _sole = sole();
        // The objection the one-way valve was built against, and the reason
        // recovery is five-clean-cases-then-one-lane rather than an immediate
        // reset: a real ceiling must be found and HELD. Here every third call
        // is refused, so the sweep never assembles a clean streak long
        // enough to climb.
        let seq = Arc::new(Mutex::new(0usize));
        let b = bench(vec![reg(&*WIDE40)], World::replies(vec![obj("a")]));
        let base = b.transport.clone();
        let seq_wrap = seq.clone();
        let busy = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let seq = seq_wrap.clone();
                Box::pin(async move {
                    let n = {
                        let mut s = seq.lock().expect("seq");
                        *s += 1;
                        *s
                    };
                    if n % 3 == 0 {
                        return Err("gateway completion 429: too many requests".into());
                    }
                    base(req).await
                })
            });
        });

        let sweep = sweep_with(&busy, "candidate", |o| {
            o.concurrency = Some(8);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;

        assert!(
            sweep
                .concurrency
                .narrowed_because
                .as_deref()
                .unwrap_or("")
                .contains("429")
        );
        assert!(sweep.concurrency.ended < 8);
    }

    #[tokio::test]
    async fn retries_a_rate_limited_fixture_instead_of_failing_it() {
        let _sole = sole();
        // A 429 is the provider saying "slower", not the model answering
        // badly. Scoring one as a contract failure is the same category error
        // as scoring a 401 as a model that cannot hold JSON.
        let call = Arc::new(Mutex::new(0usize));
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![obj("a")]));
        let base = b.transport.clone();
        let call_wrap = call.clone();
        let flaky = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let call = call_wrap.clone();
                Box::pin(async move {
                    let n = {
                        let mut c = call.lock().expect("call");
                        *c += 1;
                        *c
                    };
                    // Busy twice, then fine — the shape of a real rate limit
                    // clearing.
                    if n <= 2 {
                        return Err("gateway completion 429: rate limit exceeded".into());
                    }
                    base(req).await
                })
            });
        });

        // The production gaps are seconds; the path is what is under test,
        // not the clock, so they are injected.
        let sweep = sweep_with(&flaky, "candidate", |o| {
            o.concurrency = Some(1);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;
        let c = &sweep.cases[0];

        assert_eq!(*call.lock().expect("call"), 3);
        // The answer we eventually got is the one that counts.
        assert!(c.skipped.is_none());
        assert!(c.contract_held);
        assert_eq!(c.task, TaskVerdict::Pass);
    }

    #[tokio::test]
    async fn records_a_case_the_provider_never_let_us_ask_as_unmeasured() {
        let _sole = sole();
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![obj("a")]));
        let blocked = b.rebuilt(|r| {
            r.transport = Arc::new(|_req| {
                Box::pin(async {
                    Err::<TransportReply, String>(
                        "gateway completion 429: too many requests".into(),
                    )
                })
            });
        });

        let sweep = sweep_with(&blocked, "candidate", |o| {
            o.concurrency = Some(1);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;
        let c = &sweep.cases[0];

        // UNMEASURED, and `skipped` is what excludes it from every rate. A
        // red cell here would mean "your provider was busy" and be read as
        // "this model cannot hold a contract".
        assert!(
            c.skipped
                .as_deref()
                .unwrap_or("")
                .contains("rate limits on every attempt")
        );
        assert_eq!(c.task, TaskVerdict::Unscored);
        assert_eq!(sweep.harnesses[0].cases, 0);
        assert_eq!(sweep.harnesses[0].skipped, 1);
        // And it narrowed itself on the way, which is the other half of the
        // answer.
        assert!(
            sweep
                .concurrency
                .narrowed_because
                .as_deref()
                .unwrap_or("")
                .contains("429")
        );
    }

    #[tokio::test]
    async fn archives_every_case_for_audit_including_the_ones_that_passed() {
        let _sole = sole();
        // THE POINT OF THE ARCHIVE. The settings-row report keeps a transcript
        // only when something failed, which cannot answer the question an
        // audit actually asks — "did the model do the work, or did our
        // fixture accept something weak?" That is only answerable from a
        // PASSING transcript, and those were exactly the ones being thrown
        // away.
        let filed: Arc<Mutex<Vec<(String, String, String, bool)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let pruned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let b = bench(vec![reg(&*PICK2)], World::replies(vec![obj("a")]));
        let filed_cb = filed.clone();
        let pruned_cb = pruned.clone();

        let sweep = sweep_with(&b, "candidate", |o| {
            o.concurrency = Some(1);
            o.archive_case = Some(Arc::new(move |model, run, score| {
                let filed = filed_cb.clone();
                Box::pin(async move {
                    filed.lock().expect("filed").push((
                        model,
                        run,
                        score.case.clone(),
                        score.task == TaskVerdict::Pass,
                    ));
                })
            }));
            o.archive_prune = Some(Arc::new(move |_run| {
                let pruned = pruned_cb.clone();
                Box::pin(async move {
                    pruned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })
            }));
        })
        .await;

        assert!(sweep.cases.iter().all(|c| c.task == TaskVerdict::Pass));
        // BOTH of them, and both passed — the case the old rule discarded.
        let filed = filed.lock().expect("filed").clone();
        assert_eq!(
            filed.iter().map(|(_, _, c, _)| c.as_str()).collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert!(filed.iter().all(|(_, _, _, passed)| *passed));
        // One run identity for the whole sweep, so an auditor reads a run
        // rather than a pile of rows.
        let runs: HashSet<&String> = filed.iter().map(|(_, r, _, _)| r).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(pruned.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn files_a_resumed_sweep_under_the_original_run() {
        let _sole = sole();
        let b = bench(
            vec![reg(&*FIRST2)],
            World::replies(vec![obj("a")]).on_call(Arc::new(|call| {
                if call == 1 {
                    stop_eval_sweep(Some("candidate"));
                }
            })),
        );
        let runs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let runs_cb = runs.clone();
        let archive: Arc<dyn Fn(String, String, EvalCaseScore) -> BoxFut<()> + Send + Sync> = {
            let runs = runs_cb.clone();
            Arc::new(move |_model, run, _score| {
                let runs = runs.clone();
                Box::pin(async move {
                    runs.lock().expect("runs").push(run);
                })
            })
        };

        sweep_with(&b, "candidate", |o| {
            o.concurrency = Some(1);
            o.archive_case = Some(archive.clone());
        })
        .await;
        sweep_with(&b, "candidate", |o| {
            o.concurrency = Some(1);
            o.archive_case = Some(archive.clone());
        })
        .await;

        // An audit of "that run" has to mean one thing. A resume that opened
        // a second run identity would split the evidence in half down the
        // middle.
        let runs = runs.lock().expect("runs").clone();
        let unique: HashSet<&String> = runs.iter().collect();
        assert_eq!(unique.len(), 1);
        assert_eq!(runs.len(), 2);
    }

    #[tokio::test]
    async fn re_runs_only_the_cases_that_left_a_hole() {
        let _sole = sole();
        // The middle setting between resume and restart, and the one an admin
        // wants after a bad run: resume has nothing pending (every case is
        // recorded) and restart re-buys two hundred and forty-two cases to
        // re-ask five.
        let b = bench(vec![reg(&*PICK3_MIX)], World::replies(vec![obj("a")]));
        let first = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;
        // 'two' wants 'b' and every reply is 'a', so its CONTRACT breaks and
        // the other two pass.
        assert_eq!(
            first
                .cases
                .iter()
                .filter(|c| !c.contract_held)
                .map(|c| c.case.as_str())
                .collect::<Vec<_>>(),
            vec!["two"]
        );
        let spent = b.n_calls();

        let retried = sweep_with(&b, "candidate", |o| {
            o.concurrency = Some(1);
            o.retry_failed = true;
        })
        .await;

        // ONE CASE re-asked, not three. It costs more than one call because a
        // JSON harness gets a repair turn on a contract that did not hold —
        // the point is that 'one' and 'three' cost nothing at all.
        let re_asked = b.n_calls() - spent;
        assert!(re_asked > 0);
        assert!(re_asked < spent);
        // The ledger is still whole — all three cases, none duplicated.
        let mut names: Vec<String> = retried.cases.iter().map(|c| c.case.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["one", "three", "two"]);
        assert_eq!(retried.done, 3);
    }

    #[tokio::test]
    async fn re_runs_a_rate_limited_case_on_retry() {
        let _sole = sole();
        // The two fixtures ask for different picks, which is also how the
        // throttle below tells them apart: the rendered prompt is `pick
        // <want>`.
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let b = bench(vec![reg(&*MIXED2)], World::default());
        let busy_wrap = busy.clone();
        let gated = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let busy = busy_wrap.clone();
                Box::pin(async move {
                    // The render's ask is the FIRST line; the runner appends
                    // the JSON-contract instructions after it.
                    let ask = req
                        .messages
                        .last()
                        .and_then(|m| m.content.lines().next())
                        .unwrap_or("")
                        .to_string();
                    // 'two' is throttled on the first sweep only.
                    if busy.load(std::sync::atomic::Ordering::SeqCst) && ask.trim() == "pick b" {
                        return Err("gateway completion 429: too many requests".into());
                    }
                    let pick = ask.split(' ').nth(1).unwrap_or("a");
                    Ok(gateway_reply(json!({ "pick": pick }).to_string(), None))
                })
            });
        });

        let first = sweep_with(&gated, "candidate", |o| {
            o.concurrency = Some(1);
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;
        assert!(
            first
                .cases
                .iter()
                .find(|c| c.case == "two")
                .unwrap()
                .skipped
                .as_deref()
                .unwrap_or("")
                .contains("rate limits")
        );

        busy.store(false, std::sync::atomic::Ordering::SeqCst);
        let retried = sweep_with(&gated, "candidate", |o| {
            o.concurrency = Some(1);
            o.retry_failed = true;
            o.pressure_backoff_ms = Some(vec![1, 1, 1]);
        })
        .await;
        let two = retried.cases.iter().find(|c| c.case == "two").unwrap();
        assert!(two.skipped.is_none());
        assert_eq!(two.task, TaskVerdict::Pass);
    }

    #[tokio::test]
    async fn records_when_each_case_started_and_what_it_cost() {
        let _sole = sole();
        // `latencyMs` is the runner's measure of the FINAL attempt and has to
        // stay exactly what production records — it is what
        // observed-vs-tested compares. So it cannot answer either question a
        // speed comparison asks: what did the case cost (retries included),
        // and what was running alongside it.
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![Reply::Hang, obj("a")]));

        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_millis() as i64;
        let sweep = sweep_with(&b, "candidate", |o| {
            o.case_timeout_ms = Some(40);
            o.pressure_backoff_ms = Some(vec![1]);
        })
        .await;
        let c = &sweep.cases[0];

        // The first request vanished and was re-asked, so the case cost the
        // sweep far more than the surviving attempt's latency says.
        assert_eq!(b.n_calls(), 2);
        assert!(c.wall_ms >= 40, "wallMs {} must cover the lost request", c.wall_ms);
        assert!(c.wall_ms > c.latency_ms);
        // And it is placeable on a timeline.
        let started = chrono::DateTime::parse_from_rfc3339(&c.started_at)
            .expect("startedAt is ISO")
            .with_timezone(&chrono::Utc)
            .timestamp_millis();
        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_millis() as i64;
        assert!(started >= before - 1 && started <= after + 1);
    }

    #[tokio::test]
    async fn supplements_runs_only_fixtures_no_archived_run_has_answered() {
        let _sole = sole();
        // The mode that matters once a suite is under active development.
        // The registry gained fixtures on nine harnesses this month; a model
        // tested before them had no verdict on any. Resume cannot help (the
        // run is done, so nothing is pending) and restart re-buys everything
        // to ask the new ones.
        let b = bench(vec![reg(&*PICK2)], World::replies(vec![obj("a")]));
        let first = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;
        assert_eq!(first.done, 2);
        let spent = b.n_calls();

        // A third fixture appears in the registry.
        let grown = b.rebuilt(move |r| r.harnesses = vec![reg(&*PICK3)]);

        let after = sweep_with(&grown, "candidate", |o| {
            o.concurrency = Some(1);
            o.supplement = true;
        })
        .await;

        // ONE call for the one new question.
        assert_eq!(b.n_calls() - spent, 1);
        assert_eq!(after.done, 3);
        let mut names: Vec<String> = after.cases.iter().map(|c| c.case.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["one", "three", "two"]);
    }

    #[tokio::test]
    async fn prunes_a_verdict_about_a_fixture_that_no_longer_exists() {
        let _sole = sole();
        // A recorded case whose assertion has been deleted still scores the
        // matrix, which means the model is being judged on a question the
        // suite stopped asking. A supplemental pass is exactly the pass whose
        // subject is the difference between the ledger and the registry.
        let b = bench(vec![reg(&*PICK3)], World::replies(vec![obj("a")]));
        sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;

        let shrunk = b.rebuilt(move |r| r.harnesses = vec![reg(&*PICK2)]);
        let after = sweep_with(&shrunk, "candidate", |o| {
            o.concurrency = Some(1);
            o.supplement = true;
        })
        .await;

        let mut names: Vec<String> = after.cases.iter().map(|c| c.case.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["one", "two"]);
        assert_eq!(after.done, 2);
    }

    #[tokio::test]
    async fn measures_speed_over_the_cases_this_pass_ran() {
        let _sole = sole();
        // Otherwise a supplemental pass of one fixture would report a latency
        // computed from two hundred and forty inherited cases measured last
        // week at a different width — a number about neither this pass nor
        // this deployment.
        let b = bench(vec![reg(&*PICK2)], World::replies(vec![obj("a")]));
        let first = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;
        assert_eq!(first.measured.len(), 2);

        let grown = b.rebuilt(move |r| r.harnesses = vec![reg(&*PICK3)]);
        let after = sweep_with(&grown, "candidate", |o| {
            o.concurrency = Some(1);
            o.supplement = true;
        })
        .await;

        // The ledger is whole; the MEASUREMENT is just the new one.
        assert_eq!(after.cases.len(), 3);
        assert_eq!(
            after.measured.iter().map(|c| c.case.as_str()).collect::<Vec<_>>(),
            vec!["three"]
        );
    }

    #[tokio::test]
    async fn supplements_a_capability_the_model_lacks_and_the_install_has() {
        let _sole = sole();
        // THE HALF THAT WAS MISSING. `RoleFloor.suppliable` already let the
        // run proceed when a tool could stand in — but the sweep then handed
        // that model the ordinary transport with no tool on it, so it
        // answered from memory and the fixture failed it for having no
        // sources. Production picks the tool transport for exactly this
        // case; now the benchmark does too, so it measures what an admin
        // assigns rather than the bare weights.
        let offered: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let turn = Arc::new(Mutex::new(0usize));
        let offered_t = offered.clone();
        let turn_t = turn.clone();
        let transport: TransportFn = Arc::new(move |req| {
            let offered = offered_t.clone();
            let turn = turn_t.clone();
            Box::pin(async move {
                offered
                    .lock()
                    .expect("offered")
                    .push(req.tool_defs.iter().map(|t| t.name.clone()).collect());
                let n = {
                    let mut t = turn.lock().expect("turn");
                    *t += 1;
                    *t
                };
                // First turn: call the tool. Second: answer from what came
                // back.
                if n == 1 {
                    Ok(TransportReply {
                        kind: TransportKind::Gateway,
                        text: String::new(),
                        tool_names: Vec::new(),
                        tool_calls: Some(vec![ToolCall {
                            name: "web_search".into(),
                            id: None,
                            args: json!({ "query": "shipped this week" }).to_string(),
                        }]),
                        usage: None,
                        contract_dropped: false,
                    })
                } else {
                    Ok(TransportReply {
                        kind: TransportKind::Gateway,
                        text: "The ledger migration shipped on Friday.".into(),
                        tool_names: Vec::new(),
                        tool_calls: None,
                        usage: None,
                        contract_dropped: false,
                    })
                }
            })
        });
        let supplier = Supplier {
            server: "talaria".into(),
            tool: "web_search".into(),
        };
        let b = bench_ex(
            vec![reg(&*SEARCH_TOOL_OK)],
            World {
                missing: vec!["search".into()],
                ..Default::default()
            },
            &Extra {
                transport: Some(transport),
                supplier: Some(supplier.clone()),
                search_tool: None,
                reach: HashMap::from([(
                    "search".to_string(),
                    Reach {
                        capability: "search".into(),
                        reached: true,
                        via: Some(ReachVia::Tool),
                        supplier: Some(supplier),
                        detail: "a registered web-search server".into(),
                    },
                )]),
            },
        );

        let sweep = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;

        // THE TOOL WAS ON THE REQUEST. Without this the model is handed
        // nothing and the fixture measures whether it can guess.
        let offered = offered.lock().expect("offered").clone();
        assert!(!offered.is_empty());
        assert_eq!(offered[0], vec!["web_search".to_string()]);
        // And the case reached a verdict rather than being refused by the
        // floor.
        assert!(sweep.cases[0].skipped.is_none());
        assert!(sweep.cases[0].answered);
    }

    #[tokio::test]
    async fn files_our_gap_when_the_supplied_search_tool_finds_nothing() {
        let _sole = sole();
        // THE INVERSION THIS FIXES. Asked what NIST 800-53 AC-2 requires, a
        // model called the search tool, got back nothing citable, and then
        // did exactly what the harness asks — refused to supply the control
        // text from memory. The sweep scored that a task failure. The better
        // the model behaved, the worse it scored.
        let turn = Arc::new(Mutex::new(0usize));
        let turn_t = turn.clone();
        let transport: TransportFn = Arc::new(move |_req| {
            let turn = turn_t.clone();
            Box::pin(async move {
                let n = {
                    let mut t = turn.lock().expect("turn");
                    *t += 1;
                    *t
                };
                // Calls the tool, then honestly reports that nothing came
                // back.
                if n == 1 {
                    Ok(TransportReply {
                        kind: TransportKind::Gateway,
                        text: String::new(),
                        tool_names: Vec::new(),
                        tool_calls: Some(vec![ToolCall {
                            name: "web_search".into(),
                            id: None,
                            args: json!({ "query": "AC-2" }).to_string(),
                        }]),
                        usage: None,
                        contract_dropped: false,
                    })
                } else {
                    Ok(TransportReply {
                        kind: TransportKind::Gateway,
                        text:
                            "The results did not answer the question and I will not supply it from \
                             memory."
                                .into(),
                        tool_names: Vec::new(),
                        tool_calls: None,
                        usage: None,
                        contract_dropped: false,
                    })
                }
            })
        });
        let supplier = Supplier {
            server: "talaria".into(),
            tool: "web_search".into(),
        };
        // The tool answers, and finds NOTHING citable — which is what a
        // CAPTCHA-walled search backend looks like from in here.
        let search_tool: CallToolFn = Arc::new(|_server, _tool, _args| {
            Box::pin(async {
                Ok(ToolOutput {
                    text: "No results for that query.".into(),
                    structured: None,
                })
            })
        });
        let b = bench_ex(
            vec![reg(&*SEARCH_GAP)],
            World {
                missing: vec!["search".into()],
                ..Default::default()
            },
            &Extra {
                transport: Some(transport),
                supplier: Some(supplier.clone()),
                search_tool: Some(search_tool),
                reach: HashMap::from([(
                    "search".to_string(),
                    Reach {
                        capability: "search".into(),
                        reached: true,
                        via: Some(ReachVia::Tool),
                        supplier: Some(supplier),
                        detail: "a registered web-search server".into(),
                    },
                )]),
            },
        );

        let sweep = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;

        let one = &sweep.cases[0];
        assert!(one.gap.as_deref().unwrap_or("").contains("returned nothing citable"));
        // NOT scored against the model: a gap is unscored, and `taskError`
        // is cleared so nothing downstream reads it as a wrong answer.
        assert_eq!(one.task, TaskVerdict::Unscored);
        assert!(one.task_error.is_none());
    }

    #[tokio::test]
    async fn does_not_supplement_when_the_model_already_has_the_capability() {
        let _sole = sole();
        // The floor only asks the install for a stand-in when the model
        // cannot do it itself. Here it can, so nothing is offered — and the
        // harness runs the way production would run it.
        let offered: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let offered_t = offered.clone();
        let transport: TransportFn = Arc::new(move |req| {
            let offered = offered_t.clone();
            Box::pin(async move {
                offered
                    .lock()
                    .expect("offered")
                    .push(req.tool_defs.iter().map(|t| t.name.clone()).collect());
                Ok(TransportReply {
                    kind: TransportKind::Gateway,
                    text: "from memory, probably the ledger thing".into(),
                    tool_names: Vec::new(),
                    tool_calls: None,
                    usage: None,
                    contract_dropped: false,
                })
            })
        });
        let b = bench_ex(
            vec![reg(&*SEARCH_MEM)],
            World::default(),
            &Extra {
                transport: Some(transport),
                supplier: None,
                search_tool: None,
                reach: HashMap::new(),
            },
        );

        sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;
        let offered = offered.lock().expect("offered").clone();
        assert!(!offered.is_empty());
        assert!(offered[0].is_empty());
    }

    #[tokio::test]
    async fn records_a_floor_refusal_as_a_skip_not_a_failed_fixture() {
        let _sole = sole();
        // THE RUN THAT FOUND THIS. The health view showed a model failing five
        // `research-search` fixtures on a refusal it never saw, recorded as
        // five wrong answers. The capability floor exists to make "we did not
        // ask" visible, and the code reading its verdict was turning that
        // into "the model got it wrong".
        //
        // The floor consults the MEASURED sheet when it refuses, so this
        // world answers `search: false` from a probe — which is also why the
        // default world returns no facts at all rather than a sheet of trues.
        // And the capability is not suppliable here, so the install cannot
        // stand a tool in for it and the refusal is final.
        let b = bench(
            vec![reg(&*SEARCH_REFUSE)],
            World {
                missing: vec!["search".into()],
                capability_false: true,
                ..Default::default()
            },
        );

        let sweep = sweep_with(&b, "candidate", |o| o.concurrency = Some(1)).await;

        let one = &sweep.cases[0];
        // A SKIP carrying the runner's own refusal sentence, which every
        // consumer already reads as "no evidence" — not a contract failure
        // with a sentence about the model attached.
        assert!(one.skipped.as_deref().unwrap_or("").contains("cannot run harness"));
        assert_eq!(one.task, TaskVerdict::Unscored);
        // And the harness's own column counts it as an absence rather than a
        // case.
        assert_eq!(sweep.harnesses[0].cases, 0);
        assert_eq!(sweep.harnesses[0].skipped, 1);
    }

    #[tokio::test]
    async fn records_an_unreachable_model_as_unmeasured_not_as_a_failure() {
        let _sole = sole();
        // THE RUN THAT FOUND THIS. A sweep recorded 247 failures, every one
        // `No allowed providers are available for the selected model`, in
        // 58ms each — the org's no-train policy pinned `provider.only` to
        // the US pool and that model is served only by alibaba. A real
        // finding, reported as a model that fails every harness.
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![obj("a")]));
        let blocked = b.rebuilt(|r| {
            r.transport = Arc::new(|_req| {
                Box::pin(async {
                    Err::<TransportReply, String>(
                        "gateway completion 404: {\"error\":{\"message\":\"No allowed providers \
                         are available for the selected model.\"}}"
                            .into(),
                    )
                })
            });
        });

        let sweep = sweep_with(&blocked, "candidate", |o| o.concurrency = Some(1)).await;
        let c = &sweep.cases[0];

        assert!(c.skipped.as_deref().unwrap_or("").contains("could not reach this model"));
        assert_eq!(c.task, TaskVerdict::Unscored);
        // Excluded from every rate, because nothing about the model was
        // measured.
        assert_eq!(sweep.harnesses[0].cases, 0);
        assert_eq!(sweep.harnesses[0].skipped, 1);
    }

    #[tokio::test]
    async fn stops_after_a_streak_of_unreachable_cases() {
        let _sole = sole();
        // Two hundred and forty more cases each buy the same 404 and tell an
        // admin nothing they did not know by case three.
        let calls = Arc::new(Mutex::new(0usize));
        let b = bench(
            vec![
                reg(&*H0),
                reg(&*H1),
                reg(&*H2),
                reg(&*H3),
                reg(&*H4),
                reg(&*H5),
                reg(&*H6),
                reg(&*H7),
            ],
            World::replies(vec![obj("a")]),
        );
        let calls_cb = calls.clone();
        let blocked = b.rebuilt(move |r| {
            r.transport = Arc::new(move |_req| {
                let calls = calls_cb.clone();
                Box::pin(async move {
                    *calls.lock().expect("calls") += 1;
                    Err::<TransportReply, String>(
                        "gateway completion 404: No allowed providers are available for the \
                         selected model."
                            .into(),
                    )
                })
            });
        });

        let sweep = sweep_with(&blocked, "candidate", |o| o.concurrency = Some(1)).await;

        assert!(*calls.lock().expect("calls") <= 4);
        assert!(sweep.cases.len() < 8);
        // And it says why ONCE, rather than leaving it to be inferred from a
        // wall of identical case errors.
        assert!(sweep.error.as_deref().unwrap_or("").contains("could not reach this model"));
    }

    #[tokio::test]
    async fn does_not_stop_on_an_isolated_refusal_among_real_results() {
        let _sole = sole();
        // One 404 among passes is a blip worth recording and ignoring; the
        // streak is what makes it a fact about the whole run.
        let n = Arc::new(Mutex::new(0usize));
        let b = bench(
            vec![reg(&*H0), reg(&*H1), reg(&*H2), reg(&*H3), reg(&*H4), reg(&*H5)],
            World::replies(vec![obj("a")]),
        );
        let base = b.transport.clone();
        let n_wrap = n.clone();
        let flaky = b.rebuilt(move |r| {
            r.transport = Arc::new(move |req| {
                let base = base.clone();
                let n = n_wrap.clone();
                Box::pin(async move {
                    let i = {
                        let mut c = n.lock().expect("n");
                        *c += 1;
                        *c
                    };
                    if i == 2 {
                        return Err("gateway completion 404: no allowed providers".into());
                    }
                    base(req).await
                })
            });
        });

        let sweep = sweep_with(&flaky, "candidate", |o| o.concurrency = Some(1)).await;
        assert_eq!(sweep.cases.len(), 6);
        assert!(sweep.error.is_none());
    }

    #[tokio::test]
    async fn names_the_harnesses_no_fixture_ever_tests() {
        let _sole = sole();
        let b = bench(vec![reg(&*BARE), reg(&*PICK_A1)], World::replies(vec![obj("a")]));

        let sweep = sweep(&b, "candidate").await;
        // Not passing and not failing — invisible, which an admin reading a
        // green matrix has to be told.
        assert_eq!(sweep.unfixtured, vec!["bare"]);
        assert_eq!(
            sweep.harnesses.iter().map(|s| s.meta.id.as_str()).collect::<Vec<_>>(),
            vec!["picker"]
        );
    }

    #[tokio::test]
    async fn prices_what_it_spent_and_carries_the_token_counts_through() {
        let _sole = sole();
        let b = bench(vec![reg(&*PICK_A1)], World::replies(vec![obj("a")]));

        let sweep = sweep_with(&b, "candidate", |o| {
            o.only = Some(vec!["picker".into()]);
        })
        .await;
        assert_eq!(sweep.harnesses[0].prompt_tokens, 40);
        assert_eq!(sweep.harnesses[0].completion_tokens, 10);
        let cost = sweep.harnesses[0].cost_usd.unwrap();
        assert!((cost - 50.0 / 1_000_000.0).abs() < 1e-12);
        assert!(!sweep.harnesses[0].estimated);
    }

    #[tokio::test]
    async fn drives_the_real_registry_every_shipped_harness_every_shipped_fixture() {
        let _sole = sole();
        // The claim tier 2 rests on is that it is a driver over the registry
        // and not a new subsystem. This is that claim, executed: the actual
        // builtin harnesses, their actual fixture inputs, through the actual
        // runner. It catches the two things a hand-written driver gets wrong
        // — a `render` that throws on its own fixture, and a harness whose
        // declaration makes it unrunnable — neither of which any per-harness
        // unit test can see.
        let registry = builtin_activity_harnesses().to_vec();
        let fixtures: usize = registry.iter().map(|h| h.def.evals.len()).sum();
        assert!(
            registry.len() >= 20 && fixtures >= 50,
            "the registry carries the product ({} harnesses, {fixtures} fixtures)",
            registry.len()
        );

        // One canned reply for every harness in the product: most contracts
        // will reject it, which is the point — a sweep against a hopeless
        // model must still finish and score every column.
        let b = bench(
            registry.clone(),
            World::replies(vec![Reply::Text("{\"nope\": true}".into())]),
        );

        let sweep = sweep_with(&b, "a-hopeless-model", |o| {
            o.case_timeout_ms = Some(5_000);
        })
        .await;

        assert_eq!(sweep.state, EvalSweepState::Done);
        assert_eq!(sweep.total as usize, fixtures);
        assert_eq!(sweep.done as usize, fixtures);
        // Every harness that declares a fixture gets a column; none is
        // skipped by a throw, a hang or a failure policy.
        let mut with_fixtures: Vec<&str> = registry
            .iter()
            .filter(|h| !h.def.evals.is_empty())
            .map(|h| h.def.id)
            .collect();
        with_fixtures.sort_unstable();
        let mut scored: Vec<&str> = sweep.harnesses.iter().map(|s| s.meta.id.as_str()).collect();
        scored.sort_unstable();
        assert_eq!(scored, with_fixtures);
        assert!(sweep.cases.iter().all(|c| !c.timed_out));
        // NO FIXTURE MAY PASS ON A FOURTEEN-CHARACTER NON-ANSWER. This is the
        // assertion that keeps `taskScore` meaning something: the reply is
        // the literal string `{"nope": true}`, and a fixture that scores it
        // as a PASS is a one-sided assertion — every real failure mode it
        // checks (too long, markdown, a question, a repeat) is satisfied by
        // saying almost nothing.
        let passed_on_garbage: Vec<String> = sweep
            .cases
            .iter()
            .filter(|c| c.task == TaskVerdict::Pass)
            .map(|c| case_key(&c.harness, &c.case))
            .collect();
        assert!(
            passed_on_garbage.is_empty(),
            "fixtures passing on a non-answer: {passed_on_garbage:?}"
        );
        // Every case that FAILED something carries the drill-down an admin
        // needs — the actual prompt and the actual reply, which is what makes
        // a red cell trustworthy instead of merely alarming.
        assert!(sweep
            .cases
            .iter()
            .filter(|c| c.task != TaskVerdict::Pass && c.skipped.is_none())
            .all(|c| c.prompt.is_some()));
    }

    #[test]
    fn reports_whether_a_harness_can_express_the_input_relational_half() {
        // `verifies` is the tell for an `optimistic` count that is a bug
        // rather than a quality score: a harness with no `verify` has no way
        // to state the half of its contract a schema cannot.
        assert!(!meta_of(&reg(&*NAKED)).verifies);
        assert!(meta_of(&reg(&*PICK_A1)).verifies);

        // And whether a repair turn is even reachable, which follows
        // `maxRepairs` in the runner rather than a second reading of the same
        // rule.
        assert!(meta_of(&reg(&*NAKED)).repairable);
        assert!(!meta_of(&reg(&*BARE)).repairable);
    }

    // ── The clock a case races ──────────────────────────────────────────────

    #[test]
    fn bills_a_single_shot_case_one_turn_and_a_repairable_one_two() {
        assert_eq!(turns_per_case(&BARE, false, false), 1);
        assert_eq!(turns_per_case(&PICK_A1, false, false), 2);
    }

    #[test]
    fn bills_a_dry_run_for_the_whole_tool_loop() {
        // A flat 60s was a single-call budget applied to a case that drives
        // up to `MAX_TURNS` model calls plus a repair. The harnesses that
        // called the most timed out the most, and every one of those was
        // charged to the model as a contract failure.
        assert_eq!(turns_per_case(&PICK_A1, true, false), MAX_TURNS + 1);
        assert_eq!(turns_per_case(&BARE, true, false), MAX_TURNS);
    }

    #[test]
    fn gives_a_supplemented_case_the_clock_its_loop_actually_needs() {
        // A supplemented case runs inside the tool-search transport — up to
        // three search turns plus one to write the answer — and a one-turn
        // clock filed three cases as "did not finish", which reads as a hung
        // request and was really a four-turn job on a one-turn clock.
        static SEARCHER_TEXT: LazyLock<HarnessDefinition> =
            LazyLock::new(|| searcher(true, Arc::new(|_v, _ctx| CheckResult::Pass)));
        assert_eq!(turns_per_case(&SEARCHER_TEXT, false, false), 1);
        assert_eq!(turns_per_case(&SEARCHER_TEXT, false, true), 4);
    }

    // ── Skipping and retry, pure ────────────────────────────────────────────

    #[test]
    fn skip_reason_names_what_the_candidate_cannot_do() {
        let reason =
            harness_skip_reason("Picker", Some(ToolPolicy::Own), "gw/model", false, false)
                .expect("nothing can drive the loop");
        assert!(reason.contains("neither run its own nor be handed tool definitions"));
        assert!(reason.contains("nothing here is a measurement of it"));
        // A model that can be handed definitions is dry-run instead, and one
        // that runs its own loop is not asked this at all.
        assert!(harness_skip_reason("Picker", Some(ToolPolicy::Own), "m", false, true).is_none());
        assert!(harness_skip_reason("Picker", Some(ToolPolicy::Own), "m", true, false).is_none());
        assert!(harness_skip_reason("Picker", None, "m", false, false).is_none());
    }

    #[test]
    fn worth_retrying_covers_every_hole_and_nothing_else() {
        let clean = score(|_| {});
        assert!(!worth_retrying(&clean));
        assert!(worth_retrying(&score(|c| c.contract_held = false)));
        assert!(worth_retrying(&score(|c| c.task = TaskVerdict::Fail)));
        assert!(worth_retrying(&score(|c| {
            c.task = TaskVerdict::Unscored;
            c.gap = Some("the sandbox offered no run_tests tool".into());
        })));
        assert!(worth_retrying(&score(|c| {
            c.skipped = Some("rate limits".into());
        })));
    }

    // ── A gap in OUR test environment ───────────────────────────────────────

    #[test]
    fn a_gap_is_never_scored_as_the_model_failing() {
        // THE PRINCIPLE. A fixture asserts that a coding run ran the tests,
        // or that a session filed a capability gap — and each is only a fair
        // question if the run was actually given a test runner and a gap
        // tool. Where it was not, the model can do everything right and still
        // miss the assertion, and scoring that as a model failure measures
        // our fixture and calls it a capability.
        let cases = vec![
            score(|c| {
                c.case = "a".into();
            }),
            score(|c| {
                c.case = "b".into();
                c.task = TaskVerdict::Unscored;
                c.gap = Some(
                    "the sandbox offered no run_tests tool, so \"did it verify\" cannot be asked"
                        .into(),
                );
            }),
            score(|c| {
                c.case = "c".into();
                c.task = TaskVerdict::Fail;
                c.task_error = Some("left the bug in place".into());
            }),
        ];
        let s = score_harness(meta(), &cases);

        // One pass, one real failure. The gap is in NEITHER half of the
        // ratio.
        assert_eq!(s.task_score, Some(0.5));
        assert_eq!(s.gaps, 1);
        assert_eq!(
            s.gap_reasons,
            vec![
                "the sandbox offered no run_tests tool, so \"did it verify\" cannot be asked"
                    .to_string()
            ]
        );
    }

    #[test]
    fn a_gap_reports_its_reason_so_it_reaches_whoever_owns_the_harness() {
        // A gap that only decremented a denominator would be indistinguishable
        // from a fixture nobody wrote. The sentence is the point: it is a bug
        // report about the test environment, addressed to us.
        let s = score_harness(
            meta(),
            &[score(|c| {
                c.task = TaskVerdict::Unscored;
                c.gap = Some("no briefing was supplied for the item the assertion names".into());
            })],
        );

        assert!(s.task_score.is_none());
        assert!(s.gap_reasons[0].contains("no briefing was supplied"));
    }

    // ── The detail helpers ──────────────────────────────────────────────────

    #[test]
    fn a_timeout_with_no_calls_says_the_time_went_before_the_request() {
        let detail = timeout_detail(60_000, &[]);
        assert!(detail.contains("never made an upstream call at all"));
    }

    #[test]
    fn a_timeout_with_open_calls_names_the_wait() {
        let detail = timeout_detail(
            60_000,
            &[UpstreamAttempt {
                ms: 59_800,
                settled: false,
                error: None,
            }],
        );
        assert!(detail.contains("1 upstream call"));
        assert!(detail.contains("still had no reply"));
    }

    #[test]
    fn utf16_truncate_cuts_on_code_units_without_splitting_a_pair() {
        // BMP text: four ASCII code units survive whole.
        assert_eq!(utf16_truncate("abcd", 4), "abcd");
        assert_eq!(utf16_truncate("abcdef", 4), "abcd");
        // An emoji is TWO UTF-16 code units. A limit that would split the
        // pair steps back to the last whole character rather than
        // manufacturing a broken surrogate.
        let with_emoji = "a\u{1F680}"; // a + rocket
        assert_eq!(utf16_truncate(with_emoji, 3), with_emoji);
        assert_eq!(utf16_truncate(with_emoji, 2), "a");
        assert_eq!(utf16_truncate(with_emoji, 1), "a");
        assert_eq!(utf16_truncate("\u{1F680}b", 2), "\u{1F680}");
        assert_eq!(utf16_truncate("\u{1F680}b", 1), "");
    }
}

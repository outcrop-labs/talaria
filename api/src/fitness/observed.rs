// PRODUCTION TELEMETRY AS FITNESS SIGNAL — the half no external benchmark can
// give you, and the half nothing was reading.
//
// Two tables already carry it and both were written for other reasons:
//
//   `harness_runs`     one row per harness exit, from the runner: which model,
//                      which chain step actually won, did the contract hold,
//                      how many repairs, how many findings, how long. This is
//                      the OBSERVED twin of everything `fitness/evals.rs`
//                      benches, measured on real work.
//   `guard_findings`   one row per filed finding, from `record_findings`. Model
//                      is a column; the guard already aggregates by check for
//                      an agent. Aggregating by MODEL is the live
//                      confabulation rate, and nobody was doing it.
//
// THE ALERT THIS EXISTS FOR: a model that benched Ready and is running at a 12%
// repair rate in production. `divergences` is that alert, and the sample gate on
// it is load-bearing — three production runs is not a divergence, it is noise.
//
// ── TWO POPULATIONS. DO NOT ADD THEM. ───────────────────────────────────────
//
// `harness_runs.findings` and `guard_findings` rows are THE SAME EVENTS counted
// twice from different ends: the runner guards a reply, `record_findings` files
// a row per ungrounded finding, and the runner also stamps the COUNT of those
// same findings onto its `harness_runs` row. Summing them double-counts every
// harness finding in the install.
//
// They are also not the same POPULATION. `guard_findings` is broader — the
// public gateway route, chat and channel replies all file into it and none of
// them writes a `harness_runs` row — so it has no run denominator anywhere in
// the schema. That is why this module returns:
//
//   a RATE   `ObservedHarness.findings_per_run`, from `harness_runs` alone,
//            over the harness population, which is the only population with a
//            denominator. This is what the fitness verdict compares against.
//   COUNTS   `ObservedModel.guard_findings` / `guard_by_check`, from
//            `guard_findings` alone. Counts, never a rate, because dividing
//            them by harness runs would price gateway traffic against a
//            denominator that never contained it.
//
// Nothing here returns their sum.
//
// BOTH POPULATIONS ARE UNGROUNDED-ONLY, and for one reason stated in one place:
// the filing path drops grounded findings before it files, because a finding
// raised by the model repeating an identifier out of its own input is a fact
// about the INPUT, not about the model. The runner's `findings` count applies
// the same filter. So the two numbers are consistently scoped even though they
// must not be added.
//
// THE SWEEP DOES NOT APPEAR IN EITHER. `fitness/evals.rs` suppresses the
// recorder, and `fitness/probes.rs` does the same, precisely so that pressing
// Test cannot move the number Test is being compared against.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;

use crate::fitness::evals::HarnessScore;
use crate::harness::run::BoxFut;

/// Default lookback. Long enough that a weekly-ish harness (the librarian, the
/// concluder) has a sample, short enough that a model swapped out a month ago
/// stops dragging its old numbers into today's verdict.
pub const DEFAULT_WINDOW_DAYS: i64 = 30;

/// Below this, a production/bench gap is sampling noise and reporting it would
/// train people to ignore the alert.
pub const MIN_OBSERVED_RUNS: i64 = 20;

/// How far apart tested and observed must be before it is worth a line.
pub const DIVERGENCE_THRESHOLD: f64 = 0.1;

// ── Rows ─────────────────────────────────────────────────────────────────────

/// One (harness, model) pair as production actually ran it.
///
/// The three contract numbers use the SAME definitions `fitness/evals.rs` uses
/// for the benched half, deliberately and to the letter: `contract_rate` is the
/// contract holding on the first attempt, `repair_rate` is it holding at all
/// (cumulative, so it is always >= `contract_rate`), and `repaired_share` is
/// the share of runs that needed a repair and got one. Two spellings of the
/// predicate is how `harness_runs.schema_valid` and the eval fixtures came to
/// disagree once already; if the benched and observed numbers ever diverge
/// again it must be because the model changed, not the ruler.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedHarness {
    pub harness: String,
    /// None is a real, recorded outcome: nothing routed for this run.
    pub model: Option<String>,
    pub runs: i64,
    pub contract_rate: f64,
    pub repair_rate: f64,
    /// THE 12% NUMBER. Share of runs that took a repair turn and were saved by
    /// it — the one an admin watches after a swap.
    pub repaired_share: f64,
    /// UNGROUNDED guard findings per run. See the population note in the
    /// header: this is the only findings RATE in the module and the only one
    /// the verdict compares against.
    pub findings_per_run: f64,
    /// Share of runs that got the widened prompt — how often the
    /// capability-gated superpower actually fired for this model.
    pub widened_share: f64,
    /// Share of runs that recorded a failure sentence.
    pub error_rate: f64,
    /// Which fallback actually carried this harness, by count. A subsystem
    /// limping along on 'first-routable' for a month is a real finding and
    /// this is where it becomes visible.
    pub steps: Vec<ObservedStep>,
    pub latency_p50: i64,
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedStep {
    pub step: String,
    pub runs: i64,
}

/// One model's whole production footprint. The two findings figures are kept in
/// separate fields on purpose — see the header.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedModel {
    pub model: String,
    pub harness_runs: i64,
    /// From `harness_runs` only, over the harness population.
    pub harness_findings_per_run: f64,
    /// From `guard_findings` only. A COUNT — this population has no
    /// denominator.
    pub guard_findings: i64,
    pub guard_by_check: BTreeMap<String, i64>,
    /// The confabulation subset of `guard_by_check`: the checks that are claims
    /// about the model inventing something rather than leaking something.
    pub confabulation: i64,
}

/// The checks that mean "this model made something up", as opposed to
/// `secret_leak` / `pii_leak`, which mean "this model repeated something it
/// should not have". Both matter and they are different failures, so the
/// fitness page names them separately. Ids are RULES ids from the guard; the
/// test module locks them against that registry.
pub const CONFABULATION_CHECKS: [&str; 3] =
    ["zero_tool_claim", "ungrounded_ref", "fabricated_outage"];

// ── Injected edges ───────────────────────────────────────────────────────────

/// Grouped `harness_runs`, one row per (harness, model). Shaped as the SQL
/// returns it so a test can hand over rows without a database.
#[derive(Debug, Clone)]
pub struct HarnessRunGroup {
    pub harness: String,
    pub model: Option<String>,
    pub runs: i64,
    pub first_pass: i64,
    pub held: i64,
    pub repaired: i64,
    pub findings: i64,
    pub widened: i64,
    pub errors: i64,
    pub latency_p50: i64,
    /// Epoch milliseconds — `max(created_at)` over the group, so a group (which
    /// exists only where at least one row does) always carries one.
    pub last_run_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ChainStepGroup {
    pub harness: String,
    pub model: Option<String>,
    pub step: Option<String>,
    pub runs: i64,
}

#[derive(Debug, Clone)]
pub struct GuardFindingGroup {
    pub model: String,
    pub check: String,
    pub n: i64,
}

/// Every edge owns its inputs (each closure clones what it needs before
/// boxing), the same contract the runner's own dep set states.
pub type HarnessRunsFn =
    Arc<dyn Fn(i64) -> BoxFut<Result<Vec<HarnessRunGroup>, sqlx::Error>> + Send + Sync>;
pub type ChainStepsFn =
    Arc<dyn Fn(i64) -> BoxFut<Result<Vec<ChainStepGroup>, sqlx::Error>> + Send + Sync>;
pub type GuardFindingsFn =
    Arc<dyn Fn(i64) -> BoxFut<Result<Vec<GuardFindingGroup>, sqlx::Error>> + Send + Sync>;

#[derive(Clone)]
pub struct ObservedDeps {
    pub harness_runs: HarnessRunsFn,
    pub chain_steps: ChainStepsFn,
    pub guard_findings: GuardFindingsFn,
}

/// The production edges over sqlx, with the epoch-ms convention the crate uses
/// everywhere a timestamptz has to cross into a struct.
pub fn real_deps(pg: sqlx::PgPool) -> ObservedDeps {
    ObservedDeps {
        harness_runs: Arc::new({
            let pg = pg.clone();
            move |days| Box::pin(harness_run_groups(pg.clone(), days))
        }),
        chain_steps: Arc::new({
            let pg = pg.clone();
            move |days| Box::pin(chain_step_groups(pg.clone(), days))
        }),
        guard_findings: Arc::new({
            let pg = pg.clone();
            move |days| Box::pin(guard_finding_groups(pg.clone(), days))
        }),
    }
}

// AssertSqlSafe: every interpolation below is a bound parameter, not SQL text.
#[derive(Debug, sqlx::FromRow)]
struct RunGroupRow {
    harness: String,
    model: Option<String>,
    runs: i64,
    first_pass: i64,
    held: i64,
    repaired: i64,
    findings: i64,
    widened: i64,
    errors: i64,
    latency_p50: i64,
    last_run_ms: i64,
}

async fn harness_run_groups(
    pg: sqlx::PgPool,
    since_days: i64,
) -> Result<Vec<HarnessRunGroup>, sqlx::Error> {
    let sql = r#"
      select
        harness,
        model,
        count(*)::bigint as runs,
        sum(case when schema_valid and repairs = 0 then 1 else 0 end)::bigint as first_pass,
        sum(case when schema_valid then 1 else 0 end)::bigint                 as held,
        sum(case when schema_valid and repairs > 0 then 1 else 0 end)::bigint as repaired,
        sum(findings)::bigint                                             as findings,
        sum(case when widened then 1 else 0 end)::bigint                    as widened,
        sum(case when error is not null then 1 else 0 end)::bigint            as errors,
        coalesce(percentile_disc(0.5) within group (order by latency_ms), 0)::bigint as latency_p50,
        (extract(epoch from max(created_at)) * 1000)::bigint                 as last_run_ms
      from harness_runs
      where created_at > now() - ($1 || ' days')::interval
      group by harness, model
    "#;
    let rows: Vec<RunGroupRow> = sqlx::query_as(sql)
        .bind(since_days.to_string())
        .fetch_all(&pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| HarnessRunGroup {
            harness: r.harness,
            model: r.model,
            runs: r.runs,
            first_pass: r.first_pass,
            held: r.held,
            repaired: r.repaired,
            findings: r.findings,
            widened: r.widened,
            errors: r.errors,
            latency_p50: r.latency_p50,
            last_run_ms: r.last_run_ms,
        })
        .collect())
}

#[derive(Debug, sqlx::FromRow)]
struct StepGroupRow {
    harness: String,
    model: Option<String>,
    step: Option<String>,
    runs: i64,
}

async fn chain_step_groups(
    pg: sqlx::PgPool,
    since_days: i64,
) -> Result<Vec<ChainStepGroup>, sqlx::Error> {
    let sql = r#"
      select harness, model, chain_step as step, count(*)::bigint as runs
      from harness_runs
      where created_at > now() - ($1 || ' days')::interval
      group by harness, model, chain_step
    "#;
    let rows: Vec<StepGroupRow> = sqlx::query_as(sql)
        .bind(since_days.to_string())
        .fetch_all(&pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ChainStepGroup {
            harness: r.harness,
            model: r.model,
            step: r.step,
            runs: r.runs,
        })
        .collect())
}

#[derive(Debug, sqlx::FromRow)]
struct GuardGroupRow {
    model: String,
    check: String,
    n: i64,
}

async fn guard_finding_groups(
    pg: sqlx::PgPool,
    since_days: i64,
) -> Result<Vec<GuardFindingGroup>, sqlx::Error> {
    let sql = r#"
      select model, check_type as "check", count(*)::bigint as n
      from guard_findings
      where model is not null and created_at > now() - ($1 || ' days')::interval
      group by model, check_type
    "#;
    let rows: Vec<GuardGroupRow> = sqlx::query_as(sql)
        .bind(since_days.to_string())
        .fetch_all(&pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| GuardFindingGroup { model: r.model, check: r.check, n: r.n })
        .collect())
}

#[derive(Debug, Clone, Default)]
pub struct ObservedOptions {
    pub since_days: Option<i64>,
    /// Only this candidate. None returns every model that ran, which is what
    /// the matrix wants.
    pub model: Option<String>,
}

fn rate(n: f64, of: f64) -> f64 {
    if of == 0.0 {
        0.0
    } else {
        n / of
    }
}

// ── Reads ────────────────────────────────────────────────────────────────────

/// Production, per harness per model. Empty on a fresh install, and empty is
/// the correct answer there rather than a reason to hide the panel.
///
/// ADVISORY DATA BEHIND A VERDICT THAT MUST STILL RENDER: a telemetry query
/// that fails must not take the fitness page with it — the page falls back to
/// "no production data", which is exactly what an install with no rows shows.
pub async fn observed_harnesses(deps: &ObservedDeps, opts: &ObservedOptions) -> Vec<ObservedHarness> {
    let days = opts.since_days.unwrap_or(DEFAULT_WINDOW_DAYS);
    let groups = (deps.harness_runs)(days).await.unwrap_or_default();
    let steps = (deps.chain_steps)(days).await.unwrap_or_default();
    groups
        .into_iter()
        .filter(|g| match &opts.model {
            Some(want) => g.model.as_deref() == Some(want.as_str()),
            None => true,
        })
        .map(|g| {
            let steps: Vec<ObservedStep> = steps
                .iter()
                .filter(|s| {
                    s.harness == g.harness
                        && match (&s.model, &g.model) {
                            (Some(a), Some(b)) => a == b,
                            (None, None) => true,
                            _ => false,
                        }
                })
                .map(|s| ObservedStep {
                    step: s.step.clone().unwrap_or_else(|| "none".to_string()),
                    runs: s.runs,
                })
                .collect();
            // By count, descending — the fallback that actually carries the
            // harness reads first.
            let mut steps = steps;
            steps.sort_by(|a, b| b.runs.cmp(&a.runs));
            let runs = g.runs as f64;
            ObservedHarness {
                harness: g.harness,
                model: g.model,
                runs: g.runs,
                contract_rate: rate(g.first_pass as f64, runs),
                repair_rate: rate(g.held as f64, runs),
                repaired_share: rate(g.repaired as f64, runs),
                findings_per_run: rate(g.findings as f64, runs),
                widened_share: rate(g.widened as f64, runs),
                error_rate: rate(g.errors as f64, runs),
                steps,
                latency_p50: g.latency_p50,
                last_run_at: Some(crate::agent_auth::epoch_ms_to_iso(g.last_run_ms)),
            }
        })
        .collect()
}

/// Production, per model, across both tables — with the two findings figures
/// kept apart. There is deliberately no field summing them; see the header.
pub async fn observed_models(deps: &ObservedDeps, opts: &ObservedOptions) -> Vec<ObservedModel> {
    let days = opts.since_days.unwrap_or(DEFAULT_WINDOW_DAYS);
    let groups = (deps.harness_runs)(days).await.unwrap_or_default();
    let findings = (deps.guard_findings)(days).await.unwrap_or_default();

    let mut models: Vec<String> = Vec::new();
    let mut note = |m: &str| {
        if !models.iter().any(|x| x == m) {
            models.push(m.to_string());
        }
    };
    for g in &groups {
        if let Some(m) = &g.model {
            note(m);
        }
    }
    for f in &findings {
        note(&f.model);
    }

    let mut out: Vec<ObservedModel> = Vec::new();
    for model in models {
        if let Some(want) = &opts.model {
            if &model != want {
                continue;
            }
        }
        let mine: Vec<&HarnessRunGroup> =
            groups.iter().filter(|g| g.model.as_deref() == Some(model.as_str())).collect();
        let runs: i64 = mine.iter().map(|g| g.runs).sum();
        let mut guard_by_check: BTreeMap<String, i64> = BTreeMap::new();
        let mut guard_findings = 0i64;
        let mut confabulation = 0i64;
        for f in findings.iter().filter(|f| f.model == model) {
            *guard_by_check.entry(f.check.clone()).or_insert(0) += f.n;
            guard_findings += f.n;
            if CONFABULATION_CHECKS.contains(&f.check.as_str()) {
                confabulation += f.n;
            }
        }
        let harness_findings: i64 = mine.iter().map(|g| g.findings).sum();
        out.push(ObservedModel {
            model: model.clone(),
            harness_runs: runs,
            harness_findings_per_run: rate(harness_findings as f64, runs as f64),
            guard_findings,
            guard_by_check,
            confabulation,
        });
    }
    out.sort_by(|a, b| b.harness_runs.cmp(&a.harness_runs).then_with(|| a.model.cmp(&b.model)));
    out
}

// ── The baseline the verdict compares against ────────────────────────────────

/// Findings per run per HARNESS, across every model that ran it — which is what
/// "the current production baseline for that role" means: what this install
/// puts up with today, not what the candidate does.
///
/// ACROSS MODELS ON PURPOSE. Baselining a candidate against its own production
/// history would make a model that has been confabulating for a month its own
/// reference and score it clean. Weighted by runs so one harness that ran twice
/// cannot set the bar for one that ran ten thousand times.
///
/// Absent harnesses are absent, not zero: score.rs treats a missing entry as a
/// zero bar and SAYS SO in the reason, which is a different sentence from "we
/// measured zero".
pub fn guard_baseline(rows: &[ObservedHarness]) -> HashMap<String, f64> {
    let mut runs: HashMap<String, i64> = HashMap::new();
    let mut findings: HashMap<String, f64> = HashMap::new();
    for r in rows {
        *runs.entry(r.harness.clone()).or_insert(0) += r.runs;
        *findings.entry(r.harness.clone()).or_insert(0.0) += r.findings_per_run * r.runs as f64;
    }
    runs.into_iter()
        .map(|(harness, n)| (harness.clone(), rate(findings.get(&harness).copied().unwrap_or(0.0), n as f64)))
        .collect()
}

// ── Tested vs observed ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DivergenceMetric {
    Contract,
    Repair,
    Guard,
}

impl DivergenceMetric {
    pub fn as_str(self) -> &'static str {
        match self {
            DivergenceMetric::Contract => "contract",
            DivergenceMetric::Repair => "repair",
            DivergenceMetric::Guard => "guard",
        }
    }
}

/// One benched number that production does not agree with.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Divergence {
    pub harness: String,
    pub model: String,
    pub metric: DivergenceMetric,
    pub tested: f64,
    pub observed: f64,
    /// observed - tested. Negative on `contract` means production is WORSE; on
    /// `repair` and `guard`, positive means production is worse. `worse` says
    /// so without anyone having to remember which.
    pub delta: f64,
    pub worse: bool,
    pub observed_runs: i64,
    /// One sentence for the admin, naming the direction and the sample.
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct DivergenceOptions {
    pub min_runs: Option<i64>,
    pub threshold: Option<f64>,
}

fn pct(n: f64) -> String {
    format!("{}%", (n * 100.0).round() as i64)
}

fn per_run(n: f64) -> String {
    format!("{n:.2}/run")
}

/// Where the bench and production disagree, for one candidate.
///
/// BOTH DIRECTIONS ARE REPORTED, with `worse` set, because both are worth
/// knowing and only one of them is an alert: production worse than the bench is
/// the "benched Ready, running at a 12% repair rate" case; production BETTER
/// than the bench usually means the fixtures are harder than the real traffic,
/// which is a fact about the fixtures and belongs in front of whoever wrote
/// them.
///
/// `guard` compares like with like: `HarnessScore.guard_rate` and
/// `ObservedHarness.findings_per_run` are both ungrounded-only findings per
/// run, from the same guard pass and the same rules registry. It never touches
/// `guard_findings`, whose population has no run denominator — see the header.
pub fn divergences(
    model: &str,
    tested: &[HarnessScore],
    observed: &[ObservedHarness],
    opts: &DivergenceOptions,
) -> Vec<Divergence> {
    let min_runs = opts.min_runs.unwrap_or(MIN_OBSERVED_RUNS);
    let threshold = opts.threshold.unwrap_or(DIVERGENCE_THRESHOLD);
    let mut out: Vec<Divergence> = Vec::new();

    for score in tested {
        let Some(live) = observed
            .iter()
            .find(|o| o.harness == score.meta.id && o.model.as_deref() == Some(model))
        else {
            continue;
        };
        if live.runs < min_runs {
            continue;
        }

        let mut push = |metric: DivergenceMetric,
                        tested_value: f64,
                        observed_value: f64,
                        worse_when_higher: bool,
                        what: &str,
                        // Findings per run is not a percentage and printing it
                        // as one ("12% findings") would be a number with the
                        // wrong unit on an admin page.
                        show: fn(f64) -> String| {
            let delta = observed_value - tested_value;
            if delta.abs() < threshold {
                return;
            }
            let worse = if worse_when_higher { delta > 0.0 } else { delta < 0.0 };
            out.push(Divergence {
                harness: score.meta.id.clone(),
                model: model.to_string(),
                metric,
                tested: tested_value,
                observed: observed_value,
                delta,
                worse,
                observed_runs: live.runs,
                note: format!(
                    "{} benched {} at {} and production is running at {} over {} run(s): {}.",
                    score.meta.label,
                    what,
                    show(tested_value),
                    show(observed_value),
                    live.runs,
                    if worse { "worse than the bench" } else { "better than the bench" }
                ),
            });
        };

        push(
            DivergenceMetric::Contract,
            score.contract_rate,
            live.contract_rate,
            false,
            "first-try contract",
            pct,
        );
        // The share of runs the repair turn had to rescue. Derived from the
        // pair rather than carried, and exactly: `contract_rate` counts cases
        // that held with zero repairs and `repair_rate` counts cases that held
        // at all, so the difference is the cases that needed a repair and got
        // one — the same quantity `ObservedHarness.repaired_share` counts
        // directly.
        push(
            DivergenceMetric::Repair,
            (score.repair_rate - score.contract_rate).max(0.0),
            live.repaired_share,
            true,
            "repair-carried runs",
            pct,
        );
        push(
            DivergenceMetric::Guard,
            score.guard_rate,
            live.findings_per_run,
            true,
            "guard findings",
            per_run,
        );
    }

    // Worse first, then the bigger gap.
    out.sort_by(|a, b| {
        b.worse
            .cmp(&a.worse)
            .then_with(|| b.delta.abs().partial_cmp(&a.delta.abs()).unwrap_or(std::cmp::Ordering::Equal))
    });
    out
}

// PRICE AGAINST PERFORMANCE, weighed on the work this deployment actually does.
//
// THE QUESTION THIS ANSWERS. The matrix says whether a model can hold a slot.
// It does not say what holding it costs, and it treats the harness a fleet runs
// four thousand times a day exactly like the one the librarian runs on Sundays.
// So an admin looking at two green columns has no way to tell that one model is
// forty times the price of the other for a difference that lands on 2% of their
// traffic. That is the decision this module is for.
//
// ── THE TWO AXES, AND WHY THEY ARE WEIGHED THE SAME WAY ──────────────────────
//
// COST is not $/MTok. A sticker price cannot be compared across models because
// the harnesses differ enormously in shape: the concluder reads a whole session
// and writes four lines (prompt-heavy), the blurb writer reads a sentence and
// writes a paragraph (completion-heavy). Priced per token they look alike;
// priced per day they do not. So cost here is **what a day of your measured
// workload would cost on this model** — the runs production actually did, times
// the tokens a sweep actually measured, times this model's actual price.
//
// PERFORMANCE is not an average of contract rates. score.rs's header refuses to
// mint one and it is right: four harnesses' rates averaged into a scalar is a
// number with no referent. What DOES have a referent is coverage — **the share
// of your daily runs this model is Ready for**, and the shares that fall to
// Workable, Not-a-fit and never-measured beside it. Nothing is imputed: a
// harness nobody tested lands in `untested`, never in the numerator.
//
// Both axes are weighted by the same runs-per-day vector, which is what makes
// the pair a comparison rather than two unrelated charts.
//
// ── WHAT THIS MODULE WILL NOT DO ─────────────────────────────────────────────
//
// It will not fill a hole with an assumption. A model nothing prices has
// `usd_per_day: None` and is drawn off the cost axis rather than at zero. A
// harness no sweep has measured tokens for is EXCLUDED from the cost sum and
// COUNTED in `unmeasured`, so the figure is reported as a floor instead of
// quietly understating the bill. A fresh install with no production rows gets
// the uniform basis — one run of everything — and the payload says so, because
// "we assumed your traffic is flat" is a materially different claim from "this
// is your traffic".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::fitness::evals::HarnessScore;
use crate::fitness::observed::ObservedHarness;
use crate::fitness::score::{
    FitnessBand, FitnessReport, SlotBinding, SlotKind, band_order, harness_bands, slot_key,
};
use crate::fitness::surface::{FitnessIndex, FitnessIndexEntry, ModelPrice, TokenBudget};
use crate::harness::registry::RegisteredHarness;
use crate::harness::run::BoxFut;

// ── The per-harness half of an index entry ───────────────────────────────────

/// What one run measured about one harness, small enough to sit in the matrix
/// index rather than in the multi-hundred-kilobyte report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSummary {
    pub band: FitnessBand,
    pub cases: i64,
    pub prompt: i64,
    pub completion: i64,
}

/// Collapse a report and its sweep scores into that half.
///
/// ONE DEFINITION, TWO CALLERS: the index entry writer (surface.rs) writes it
/// when a run finishes, and `value_view` derives it on the fly for reports
/// archived before the field existed. Two spellings of "per-case tokens" is how
/// the archived entry and the live one would come to disagree about what a
/// model costs.
pub fn harness_summary(
    report: &FitnessReport,
    scores: &[HarnessScore],
) -> HashMap<String, HarnessSummary> {
    let swept: HashMap<&str, &HarnessScore> =
        scores.iter().map(|s| (s.meta.id.as_str(), s)).collect();
    let mut out = HashMap::new();
    for (id, band) in harness_bands(report) {
        let s = swept.get(id.as_str()).copied();
        // `cases: 0` — the report judged this harness but the sweep never called
        // it. Recorded as zero rather than divided by zero, and read downstream
        // as "no tokens of this model's own", never as "this harness is free".
        let cases = s.map_or(0, |s| s.cases);
        out.insert(
            id,
            HarnessSummary {
                band,
                cases,
                prompt: if cases > 0 {
                    (s.map_or(0, |s| s.prompt_tokens) as f64 / cases as f64).round() as i64
                } else {
                    0
                },
                completion: if cases > 0 {
                    (s.map_or(0, |s| s.completion_tokens) as f64 / cases as f64).round() as i64
                } else {
                    0
                },
            },
        );
    }
    out
}

// ── The workload ─────────────────────────────────────────────────────────────

/// WHERE THE RUNS-PER-DAY VECTOR CAME FROM, which changes how much of the rest
/// of this payload an admin should believe.
///
/// `observed` — grouped `harness_runs` over the telemetry window, summed across
/// every model that served each harness. This is the real shape of the day.
///
/// `uniform` — nothing has run yet, so every fixtured harness is counted once.
/// It makes the page useful on day one and it is NOT a measurement; the UI says
/// which basis it drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkloadBasis {
    Observed,
    Uniform,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workload {
    pub basis: WorkloadBasis,
    pub window_days: i64,
    /// harness id → runs per day.
    pub runs: BTreeMap<String, f64>,
    /// Sum of the above. Zero only on the impossible empty registry.
    pub per_day: f64,
    /// Runs per day landing on harnesses that declare no eval fixtures. Real
    /// work that no run can ever score — reported separately so the untested
    /// share can say WHY it is untested rather than reading as an admin's
    /// oversight.
    pub unfixtured_per_day: f64,
    /// Distinct harnesses with any volume.
    pub harnesses: usize,
}

pub fn workload_from(
    observed: &[ObservedHarness],
    registry: &[RegisteredHarness],
    window_days: i64,
) -> Workload {
    let days = window_days.max(1) as f64;
    let known: HashSet<&str> = registry.iter().map(|h| h.def.id).collect();
    let mut runs: BTreeMap<String, f64> = BTreeMap::new();

    for row in observed {
        // A harness that has since been deleted from the registry still has rows
        // in `harness_runs`. Counting it would put volume in the denominator
        // that no model can ever be scored on, which reads as every model
        // getting worse.
        if !known.contains(row.harness.as_str()) {
            continue;
        }
        *runs.entry(row.harness.clone()).or_insert(0.0) += row.runs as f64 / days;
    }

    let observed_total: f64 = runs.values().sum();
    if observed_total <= 0.0 {
        // THE UNIFORM BASIS deliberately covers only fixtured harnesses. On the
        // observed basis an unfixtured harness with real traffic is a real hole
        // and is reported as one; inventing traffic for it here would
        // manufacture the hole instead of finding it.
        let fixtured: Vec<&RegisteredHarness> = registry
            .iter()
            .filter(|h| !h.def.evals.is_empty())
            .collect();
        for h in &fixtured {
            runs.insert(h.def.id.to_string(), 1.0);
        }
        return Workload {
            basis: WorkloadBasis::Uniform,
            window_days: days as i64,
            runs,
            per_day: fixtured.len() as f64,
            unfixtured_per_day: 0.0,
            harnesses: fixtured.len(),
        };
    }

    let unfixtured: HashSet<&str> = registry
        .iter()
        .filter(|h| h.def.evals.is_empty())
        .map(|h| h.def.id)
        .collect();
    let unfixtured_per_day = runs
        .iter()
        .filter(|(id, _)| unfixtured.contains(id.as_str()))
        .map(|(_, v)| *v)
        .sum();
    let harnesses = runs.len();
    Workload {
        basis: WorkloadBasis::Observed,
        window_days: days as i64,
        runs,
        per_day: observed_total,
        unfixtured_per_day,
        harnesses,
    }
}

// ── What one model's last run says about each harness ────────────────────────

/// Per-case tokens for one (model, harness), and WHOSE verbosity they measure.
///
/// `model` — this model's own sweep measured them. The only basis that prices a
/// terse model and a chatty one differently, which is most of what separates
/// their bills.
///
/// `shared` — the harness's entry in the global token budget, measured from
/// whichever candidate last swept it. Right about the prompt (the fixtures are
/// fixed) and only approximately right about the completion. Reports written
/// before this view existed have no per-model tokens and land here.
///
/// `none` — nothing has ever measured this harness. Excluded from the sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenBasis {
    Model,
    Shared,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HarnessTokens {
    pub prompt: i64,
    pub completion: i64,
    pub basis: TokenBasis,
}

pub fn tokens_for(
    harness: &str,
    entry: Option<&FitnessIndexEntry>,
    budget: &TokenBudget,
) -> HarnessTokens {
    // ZERO IS NOT A MEASUREMENT anywhere in this chain. A run that failed every
    // case before a token moved records 0/0, and reading that as "this harness
    // is free" would print a confident $0.00 for a model nobody has priced
    // anything about. The budget writer no longer records such an entry; this
    // also declines to read the ones already on disk.
    let own = entry
        .and_then(|e| e.harnesses.as_ref())
        .and_then(|h| h.get(harness));
    if let Some(own) = own
        && own.cases > 0
        && own.prompt + own.completion > 0
    {
        return HarnessTokens {
            prompt: own.prompt,
            completion: own.completion,
            basis: TokenBasis::Model,
        };
    }
    if let Some(shared) = budget.get(harness)
        && shared.prompt + shared.completion > 0
    {
        return HarnessTokens {
            prompt: shared.prompt,
            completion: shared.completion,
            basis: TokenBasis::Shared,
        };
    }
    HarnessTokens {
        prompt: 0,
        completion: 0,
        basis: TokenBasis::None,
    }
}

/// THE BAND THIS MODEL EARNED ON THIS HARNESS.
///
/// Prefers `entry.harnesses`, which the index writer collapses out of the
/// report at write time and is the only source that can speak for an UNBOUND
/// harness (no slot, therefore no cell).
///
/// Falls back to the cells for entries written before that field existed: worst
/// band across the slots this harness is bound to, the same reduction
/// `harness_bands` does. An old entry therefore reports its bound harnesses
/// correctly and its unbound ones as `untested` — understating a model rather
/// than flattering it, which is the right direction to be wrong in.
pub fn band_for(
    harness: &str,
    entry: Option<&FitnessIndexEntry>,
    slots_of: &HashMap<String, Vec<String>>,
) -> FitnessBand {
    let Some(entry) = entry else {
        return FitnessBand::Untested;
    };
    if let Some(own) = entry.harnesses.as_ref().and_then(|h| h.get(harness)) {
        return own.band;
    }
    let mut worst: Option<FitnessBand> = None;
    for key in slots_of.get(harness).into_iter().flatten() {
        let Some(band) = entry.cells.get(key).map(|c| c.band) else {
            continue;
        };
        if worst.is_none_or(|w| band_order(band) < band_order(w)) {
            worst = Some(band);
        }
    }
    worst.unwrap_or(FitnessBand::Untested)
}

/// harness id → the slot keys it is bound to. Inverted once per read.
pub fn slots_by_harness(bindings: &[SlotBinding]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for b in bindings {
        let key = slot_key(b.slot.kind, &b.slot.id);
        for h in &b.harnesses {
            out.entry(h.id.clone()).or_default().push(key.clone());
        }
    }
    out
}

// ── One model's row ──────────────────────────────────────────────────────────

/// Share of daily runs by band, in the payload's shape: a flat record keyed
/// by the band's lowercase name. Sums to 1 across the workload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct BandShares {
    pub ready: f64,
    pub workable: f64,
    pub unfit: f64,
    pub untested: f64,
    pub unbound: f64,
}

impl BandShares {
    fn add(&mut self, band: FitnessBand, v: f64) {
        match band {
            FitnessBand::Ready => self.ready += v,
            FitnessBand::Workable => self.workable += v,
            FitnessBand::Unfit => self.unfit += v,
            FitnessBand::Untested => self.untested += v,
            FitnessBand::Unbound => self.unbound += v,
        }
    }

    /// Only the tests read the total — the payload carries the five shares, and
    /// the one assertion that they cover the day belongs beside them.
    #[cfg(test)]
    fn sum(&self) -> f64 {
        self.ready + self.workable + self.unfit + self.untested + self.unbound
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelValue {
    pub model: String,
    /// When the run behind these bands finished. None means never tested, and
    /// the row is still emitted — "you are paying this much for a model nobody
    /// has measured" is one of the more useful things this table can say.
    pub at: Option<String>,
    pub price: Option<ModelPrice>,
    /// Your measured day, priced on this model. None when nothing prices it.
    pub usd_per_day: Option<f64>,
    /// THE PRICE-TO-PERFORMANCE NUMBER, and the one with an actual referent:
    /// dollars per run this model is trusted to do. A cheap model that is Ready
    /// for a tenth of your day is not cheap. None when the cost is none or the
    /// ready share is zero — a division nobody can act on.
    pub usd_per_ready_run: Option<f64>,
    pub shares: BandShares,
    pub ready_share: f64,
    /// Ready or Workable — what the model can carry, with a repair turn allowed.
    pub usable_share: f64,
    /// Share of daily runs whose tokens something has measured. Below 1 the
    /// cost is a FLOOR, and the UI says so rather than printing a confident
    /// total.
    pub cost_coverage: f64,
    /// Whose verbosity `usd_per_day` was computed from. `model` only when every
    /// priced harness used this model's own sweep.
    pub token_basis: TokenBasis,
}

pub fn value_of(
    model: &str,
    entry: Option<&FitnessIndexEntry>,
    price: Option<ModelPrice>,
    workload: &Workload,
    budget: &TokenBudget,
    slots_of: &HashMap<String, Vec<String>>,
) -> ModelValue {
    let mut shares = BandShares::default();
    let mut usd = 0.0;
    let mut priced_runs = 0.0;
    let mut saw_shared = false;
    let mut saw_own = false;

    for (harness, per_day) in &workload.runs {
        if *per_day <= 0.0 {
            continue;
        }
        shares.add(band_for(harness, entry, slots_of), *per_day);
        let tokens = tokens_for(harness, entry, budget);
        if tokens.basis == TokenBasis::None {
            continue;
        }
        match tokens.basis {
            TokenBasis::Model => saw_own = true,
            TokenBasis::Shared => saw_shared = true,
            TokenBasis::None => {}
        }
        priced_runs += per_day;
        if let Some(price) = price {
            usd += (per_day
                * ((tokens.prompt as f64) * price.in_per_mtok
                    + (tokens.completion as f64) * price.out_per_mtok))
                / 1e6;
        }
    }

    let total = workload.per_day;
    if total > 0.0 {
        shares.ready /= total;
        shares.workable /= total;
        shares.unfit /= total;
        shares.untested /= total;
        shares.unbound /= total;
    }

    // A PRICE WITH NOTHING TO PRICE IS NOT $0. The gemma run that failed every
    // case has a perfectly good $/MTok and not one measured token, and reporting
    // that as "$0 a day" would put the most expensive-looking model on the page
    // at the cheap end of the chart. No measurement, no figure.
    let usd_per_day = if price.is_none() || priced_runs == 0.0 {
        None
    } else {
        Some(usd)
    };
    let ready_runs = shares.ready * total;
    ModelValue {
        model: model.to_string(),
        at: entry.map(|e| e.at.clone()),
        price,
        usd_per_day,
        usd_per_ready_run: match usd_per_day {
            Some(usd) if ready_runs > 0.0 => Some(usd / ready_runs),
            _ => None,
        },
        ready_share: shares.ready,
        usable_share: shares.ready + shares.workable,
        cost_coverage: if total > 0.0 {
            priced_runs / total
        } else {
            0.0
        },
        token_basis: if saw_own && !saw_shared {
            TokenBasis::Model
        } else if saw_own || saw_shared {
            TokenBasis::Shared
        } else {
            TokenBasis::None
        },
        shares,
    }
}

// ── One slot's row ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotCandidate {
    pub model: String,
    pub band: FitnessBand,
    pub usd_per_day: Option<f64>,
}

/// A slot, its share of the day, and what each model that can hold it costs to
/// run IT — not the whole workload. An admin choosing the Research model wants
/// the Research bill, and a model's whole-day cost is dominated by whatever
/// harness happens to run most.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotValue {
    pub key: String,
    pub label: String,
    pub kind: SlotKind,
    pub live: bool,
    /// Runs per day across the harnesses bound to this slot. A harness bound to
    /// two slots counts in both: this is per-slot demand, not a partition, and
    /// summing the column would double-count on purpose-shared harnesses.
    pub per_day: f64,
    pub harnesses: usize,
    /// Every model that reaches Workable or better, cheapest first with Ready
    /// ahead of Workable. Empty means nothing tested can hold this slot.
    pub candidates: Vec<SlotCandidate>,
    /// The cheapest Ready candidate — the actual recommendation. None when none
    /// is Ready, which is a finding rather than a gap.
    pub best: Option<String>,
}

// ── The read ─────────────────────────────────────────────────────────────────

/// The archived report, read ONLY to backfill an index entry written before it
/// carried its per-harness half — see `backfill`.
pub struct ArchivedRecord {
    pub report: FitnessReport,
    pub harnesses: Vec<HarnessScore>,
}

pub type ObservedFn = Arc<dyn Fn() -> BoxFut<Result<Vec<ObservedHarness>, String>> + Send + Sync>;
pub type HarnessesFn =
    Arc<dyn Fn() -> BoxFut<Result<Vec<RegisteredHarness>, String>> + Send + Sync>;
pub type BindingsFn =
    Arc<dyn Fn(Vec<RegisteredHarness>) -> BoxFut<Result<Vec<SlotBinding>, String>> + Send + Sync>;
pub type IndexFn = Arc<dyn Fn() -> BoxFut<Result<FitnessIndex, String>> + Send + Sync>;
pub type BudgetFn = Arc<dyn Fn() -> BoxFut<Result<TokenBudget, String>> + Send + Sync>;
pub type PriceFn = Arc<dyn Fn(String) -> BoxFut<Result<Option<ModelPrice>, String>> + Send + Sync>;
pub type RecordFn =
    Arc<dyn Fn(String) -> BoxFut<Result<Option<ArchivedRecord>, String>> + Send + Sync>;

pub struct ValueDeps {
    pub observed: ObservedFn,
    pub harnesses: HarnessesFn,
    pub bindings: BindingsFn,
    pub index: IndexFn,
    pub budget: BudgetFn,
    pub price: PriceFn,
    /// The archived report, read ONLY to backfill an index entry written before
    /// it carried its per-harness half. An error (or None) leaves that model on
    /// the cells and the shared budget, which is what it would have had anyway.
    pub record: RecordFn,
    pub window_days: i64,
}

/// FILL IN WHAT AN OLDER RUN ALREADY MEASURED.
///
/// Every number on this page needs per-harness bands and per-harness tokens.
/// New runs write them into the index; reports archived before that field
/// existed have them only in the full record — which is real, paid-for
/// measurement, and stranding it behind "re-test this model" would be asking an
/// admin to buy a sweep twice.
///
/// BOUNDED AND SELF-LIMITING: at most one read per index entry MISSING the
/// field, at most the archive-keep cap of those, and each one stops needing it
/// the next time that model is tested. Deliberately not written back — a GET
/// that rewrites the archive is a surprise, and the read it saves is one an
/// admin pays only while old entries survive eviction.
async fn backfill(index: FitnessIndex, record: &RecordFn) -> FitnessIndex {
    let stale: Vec<String> = index
        .iter()
        .filter(|(_, e)| e.harnesses.is_none())
        .map(|(k, _)| k.clone())
        .collect();
    if stale.is_empty() {
        return index;
    }
    let mut filled = index;
    for model in stale {
        let Some(entry) = filled.get(&model) else {
            continue;
        };
        // `entry.model`, not the KEY: the index is keyed by the id the catalog
        // offers and the report is filed under the id the run used. See
        // `stored_id_for` in surface.rs.
        let read_model = entry.model.clone();
        let Some(rec) = (record)(read_model).await.ok().flatten() else {
            continue;
        };
        if let Some(e) = filled.get_mut(&model) {
            e.harnesses = Some(harness_summary(&rec.report, &rec.harnesses));
        }
    }
    filled
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueView {
    pub workload: Workload,
    pub models: Vec<ModelValue>,
    pub slots: Vec<SlotValue>,
    /// Harnesses carrying volume that no sweep has ever measured tokens for.
    /// Non-empty means every `usd_per_day` on the page is a floor.
    pub unmeasured: Vec<String>,
    /// True when at least one model priced. False turns the cost axis off
    /// rather than drawing every model at zero.
    pub priced: bool,
}

pub async fn value_view(deps: &ValueDeps) -> Result<ValueView, String> {
    let observed = (deps.observed)().await.unwrap_or_default();
    let registry = (deps.harnesses)().await?;
    let archived = (deps.index)().await?;
    let budget = (deps.budget)().await.unwrap_or_default();
    let index = backfill(archived, &deps.record).await;
    let workload = workload_from(&observed, &registry, deps.window_days);
    let bindings = (deps.bindings)(registry).await.unwrap_or_default();
    let slots_of = slots_by_harness(&bindings);

    // ONLY MODELS WITH A REPORT. A row for every id on the gateway would be four
    // hundred rows of "untested, 0% ready", which is true and is noise; the
    // matrix above is where an untested model is chosen to be tested.
    let mut models: Vec<String> = index.keys().cloned().collect();
    models.sort();
    let mut rows = Vec::with_capacity(models.len());
    for model in &models {
        let price = (deps.price)(model.clone()).await.ok().flatten();
        rows.push(value_of(
            model,
            index.get(model),
            price,
            &workload,
            &budget,
            &slots_of,
        ));
    }

    let mut slots = Vec::with_capacity(bindings.len());
    for b in &bindings {
        let key = slot_key(b.slot.kind, &b.slot.id);
        let ids: Vec<&str> = b.harnesses.iter().map(|h| h.id.as_str()).collect();
        let per_day: f64 = ids
            .iter()
            .map(|id| workload.runs.get(*id).copied().unwrap_or(0.0))
            .sum();
        let mut candidates: Vec<SlotCandidate> = Vec::new();
        for row in &rows {
            let band = index
                .get(&row.model)
                .and_then(|e| e.cells.get(&key))
                .map_or(FitnessBand::Untested, |c| c.band);
            if band != FitnessBand::Ready && band != FitnessBand::Workable {
                continue;
            }
            // This slot's own bill: the harnesses bound to it, and nothing else.
            // None unless something here was actually measured, for the same
            // reason the whole-workload figure is — see `value_of`.
            let price = row.price;
            let mut usd: Option<f64> = None;
            if let Some(price) = price {
                for id in ids.iter().copied() {
                    let t = tokens_for(id, index.get(&row.model), &budget);
                    if t.basis == TokenBasis::None {
                        continue;
                    }
                    usd = Some(
                        usd.unwrap_or(0.0)
                            + (workload.runs.get(id).copied().unwrap_or(0.0)
                                * ((t.prompt as f64) * price.in_per_mtok
                                    + (t.completion as f64) * price.out_per_mtok))
                                / 1e6,
                    );
                }
            }
            candidates.push(SlotCandidate {
                model: row.model.clone(),
                band,
                usd_per_day: usd,
            });
        }
        candidates.sort_by(|a, b| {
            band_order(b.band).cmp(&band_order(a.band)).then_with(|| {
                match (a.usd_per_day, b.usd_per_day) {
                    // An unpriced candidate sorts last within its band: it may
                    // well be the cheapest, and a page that ranked it first
                    // would be guessing.
                    (None, None) => a.model.cmp(&b.model),
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                }
            })
        });
        let best = candidates
            .iter()
            .find(|c| c.band == FitnessBand::Ready)
            .map(|c| c.model.clone());
        slots.push(SlotValue {
            key,
            label: b.slot.label.clone(),
            kind: b.slot.kind,
            live: b.slot.live,
            per_day,
            harnesses: ids.len(),
            candidates,
            best,
        });
    }

    let mut measured: HashSet<String> = budget.keys().cloned().collect();
    for e in index.values() {
        if let Some(h) = &e.harnesses {
            measured.extend(h.keys().cloned());
        }
    }
    let mut unmeasured: Vec<String> = workload
        .runs
        .iter()
        .filter(|(id, runs)| **runs > 0.0 && !measured.contains(*id))
        .map(|(id, _)| id.clone())
        .collect();
    unmeasured.sort();

    Ok(ValueView {
        priced: rows.iter().any(|r| r.usd_per_day.is_some()),
        workload,
        models: rows,
        slots,
        unmeasured,
    })
}

#[cfg(test)]
mod tests {
    // 33 its, no database anywhere near them. `value_view` runs over injected
    // edges, so the whole-read tests are async; everything else is plain
    // arithmetic.
    use super::*;
    use crate::fitness::evals::{BandScores, HarnessMeta};
    use crate::fitness::score::{BindingVia, BoundHarness, FitnessSlot};
    use crate::fitness::surface::{FitnessCell, TierId};
    use crate::harness::define::{
        CheckCtx, CheckResult, EvalCase, HarnessDefinition, OnFailure, Output, RenderContext,
    };
    use crate::harness::registry::HarnessSource;
    use crate::harness::schema::Schema;
    use serde_json::{Value, json};
    use std::sync::Mutex;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} is not close to {b}");
    }

    // ── Fixtures ─────────────────────────────────────────────────────────────

    /// `value.rs` reads exactly two things off a registered harness — its id
    /// and whether it has any fixtures — so the fixture states those. Built
    /// through `HarnessDefinition::new` rather than `define_harness` on
    /// purpose, the same way score.rs's fixtures are: the derived json floor is
    /// a runtime refusal this module never consults.
    fn harness(id: &'static str, fixtures: usize) -> RegisteredHarness {
        let mut def = HarnessDefinition::new(
            id,
            id,
            "Answers.",
            crate::harness_model::ModelSpec {
                pin: None,
                role: None,
                chain: Some(&[]),
                user_id: None,
            },
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(Vec::new())),
            Output::Json {
                schema: Schema::string(),
                preprocess: None,
                repair: None,
                verify: None,
            },
            OnFailure::Null,
        );
        def.evals = (0..fixtures)
            .map(|_| {
                EvalCase::new(
                    "one",
                    json!({ "q": "x" }),
                    Arc::new(|_value: &Value, _ctx: &CheckCtx| CheckResult::Pass),
                )
            })
            .collect();
        RegisteredHarness {
            def: Box::leak(Box::new(def)),
            source: HarnessSource::Builtin,
        }
    }

    fn observed(id: &str, runs: i64) -> ObservedHarness {
        observed_by(id, runs, "a")
    }

    fn observed_by(id: &str, runs: i64, model: &str) -> ObservedHarness {
        ObservedHarness {
            harness: id.to_string(),
            model: Some(model.to_string()),
            runs,
            contract_rate: 0.0,
            repair_rate: 0.0,
            repaired_share: 0.0,
            findings_per_run: 0.0,
            widened_share: 0.0,
            error_rate: 0.0,
            steps: Vec::new(),
            latency_p50: 0,
            last_run_at: None,
        }
    }

    fn entry() -> FitnessIndexEntry {
        FitnessIndexEntry {
            model: "m".to_string(),
            at: "2026-08-01T00:00:00.000Z".to_string(),
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

    fn cell(band: FitnessBand) -> FitnessCell {
        FitnessCell { band, reason: None }
    }

    fn summary(band: FitnessBand, cases: i64, prompt: i64, completion: i64) -> HarnessSummary {
        HarnessSummary {
            band,
            cases,
            prompt,
            completion,
        }
    }

    fn slot_fixture(kind: SlotKind, id: &str) -> FitnessSlot {
        FitnessSlot {
            kind,
            id: id.to_string(),
            label: id.to_string(),
            hint: String::new(),
            requires: Vec::new(),
            live: true,
        }
    }

    fn slot_binding(kind: SlotKind, id: &str, harnesses: &[&str]) -> SlotBinding {
        slot_binding_as(kind, id, harnesses, id)
    }

    fn slot_binding_as(kind: SlotKind, id: &str, harnesses: &[&str], label: &str) -> SlotBinding {
        SlotBinding {
            slot: FitnessSlot {
                label: label.to_string(),
                ..slot_fixture(kind, id)
            },
            harnesses: harnesses
                .iter()
                .map(|h| BoundHarness {
                    id: h.to_string(),
                    via: BindingVia::Chain,
                })
                .collect(),
        }
    }

    fn verdict(id: &str, band: FitnessBand) -> crate::fitness::score::HarnessVerdict {
        crate::fitness::score::HarnessVerdict {
            harness: id.to_string(),
            label: id.to_string(),
            band,
            floor: 0.8,
            contract_rate: 1.0,
            repair_rate: 1.0,
            task_score: Some(1.0),
            guard_rate: 0.0,
            guard_baseline: None,
            cases: 1,
            reasons: Vec::new(),
        }
    }

    /// A slot verdict shaped only as far as `harness_bands` reads it.
    fn slot_verdict(id: &str, band: FitnessBand) -> crate::fitness::score::SlotVerdict {
        crate::fitness::score::SlotVerdict {
            slot: slot_fixture(SlotKind::Role, id),
            band,
            reasons: Vec::new(),
            harnesses: vec![verdict(id, band)],
            task_floor: 0.8,
            contract: None,
            repair: None,
            task: None,
        }
    }

    fn score_like(id: &str, cases: i64, prompt: i64, completion: i64) -> HarnessScore {
        HarnessScore {
            meta: HarnessMeta {
                id: id.to_string(),
                label: id.to_string(),
                source: "builtin".to_string(),
                output_kind: "json".into(),
                tools: "none".into(),
                requires: Vec::new(),
                verifies: true,
                repairable: true,
            },
            cases,
            skipped: 0,
            gaps: 0,
            gap_reasons: Vec::new(),
            skip_reason: None,
            scored: cases,
            contract_rate: 1.0,
            repair_rate: 1.0,
            repair_yield: None,
            task_score: Some(1.0),
            band_scores: BandScores::default(),
            guard_rate: 0.0,
            answered_rate: 1.0,
            latency_p50: 0,
            latency_p95: 0,
            prompt_tokens: prompt,
            completion_tokens: completion,
            cost_usd: None,
            estimated: false,
            timeouts: 0,
            optimistic: 0,
        }
    }

    // ── The workload ──────────────────────────────────────────────────────────

    #[test]
    fn workload_divides_observed_runs_by_the_window_summing_across_models() {
        let w = workload_from(
            &[
                observed_by("ticket", 300, "a"),
                observed_by("ticket", 300, "b"),
                observed("brief", 30),
            ],
            &[harness("ticket", 2), harness("brief", 2)],
            30,
        );

        assert_eq!(w.basis, WorkloadBasis::Observed);
        close(w.runs["ticket"], 20.0);
        close(w.runs["brief"], 1.0);
        close(w.per_day, 21.0);
        assert_eq!(w.harnesses, 2);
    }

    #[test]
    fn workload_drops_volume_from_a_harness_the_registry_no_longer_has() {
        // Rows outlive the code. Counting a deleted harness would put runs in
        // the denominator that no model can ever be scored on, which reads on
        // the page as every model getting worse.
        let w = workload_from(
            &[observed("ticket", 30), observed("deleted-last-year", 3000)],
            &[harness("ticket", 2)],
            30,
        );

        assert_eq!(w.runs.keys().collect::<Vec<_>>(), vec!["ticket"]);
        close(w.per_day, 1.0);
    }

    #[test]
    fn workload_counts_traffic_on_an_unfixtured_harness_and_says_how_much() {
        // Real work nobody can score. It belongs in the denominator — a page
        // that dropped it would report 100% coverage of a day it only saw half
        // of.
        let w = workload_from(
            &[observed("ticket", 30), observed("unscorable", 90)],
            &[harness("ticket", 2), harness("unscorable", 0)],
            30,
        );

        close(w.per_day, 4.0);
        close(w.unfixtured_per_day, 3.0);
    }

    #[test]
    fn workload_falls_back_to_one_run_of_every_fixtured_harness_when_production_is_idle() {
        let w = workload_from(
            &[],
            &[
                harness("ticket", 2),
                harness("brief", 2),
                harness("unscorable", 0),
            ],
            30,
        );

        assert_eq!(w.basis, WorkloadBasis::Uniform);
        close(w.per_day, 2.0);
        assert!(!w.runs.contains_key("unscorable"));
        // The uniform basis invents no traffic for what it cannot score, so it
        // has no hole to report either.
        close(w.unfixtured_per_day, 0.0);
    }

    // ── Tokens ────────────────────────────────────────────────────────────────

    fn budget() -> TokenBudget {
        let mut b = TokenBudget::new();
        b.insert(
            "ticket".to_string(),
            crate::fitness::surface::TokenBudgetEntry {
                prompt: 1000,
                completion: 100,
                at: "x".to_string(),
            },
        );
        b
    }

    #[test]
    fn tokens_prefer_this_models_own_measurement_over_the_shared_budget() {
        let e = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Ready, 4, 900, 400),
            )])),
            ..entry()
        };

        assert_eq!(
            tokens_for("ticket", Some(&e), &budget()),
            HarnessTokens {
                prompt: 900,
                completion: 400,
                basis: TokenBasis::Model
            }
        );
    }

    #[test]
    fn tokens_fall_back_to_the_shared_budget_when_the_model_ran_no_cases() {
        // `cases: 0` with a band means the report judged the harness but the
        // sweep never called it. Pricing that at zero tokens would make a
        // skipped harness look free.
        let e = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Untested, 0, 0, 0),
            )])),
            ..entry()
        };

        assert_eq!(
            tokens_for("ticket", Some(&e), &budget()),
            HarnessTokens {
                prompt: 1000,
                completion: 100,
                basis: TokenBasis::Shared
            }
        );
    }

    #[test]
    fn tokens_report_nothing_measured_rather_than_zero() {
        assert_eq!(
            tokens_for("brief", Some(&entry()), &budget()),
            HarnessTokens {
                prompt: 0,
                completion: 0,
                basis: TokenBasis::None
            }
        );
    }

    #[test]
    fn tokens_refuse_to_read_an_all_zero_entry_as_a_measurement_of_zero() {
        // How this install lost its budget: a sweep against a model id the
        // gateway could not reach ran every case, failed every one before a
        // token moved, and wrote 0/0 for 26 harnesses. Read as real, it prints
        // a confident $0.00 for every model on the page.
        let mut zeroed_budget = TokenBudget::new();
        zeroed_budget.insert(
            "ticket".to_string(),
            crate::fitness::surface::TokenBudgetEntry {
                prompt: 0,
                completion: 0,
                at: "x".to_string(),
            },
        );
        assert_eq!(
            tokens_for("ticket", Some(&entry()), &zeroed_budget).basis,
            TokenBasis::None
        );

        let zeroed = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Unfit, 4, 0, 0),
            )])),
            ..entry()
        };
        assert_eq!(
            tokens_for("ticket", Some(&zeroed), &TokenBudget::new()).basis,
            TokenBasis::None
        );
        // …and still falls through to a shared budget that DOES have numbers.
        assert_eq!(
            tokens_for("ticket", Some(&zeroed), &budget()),
            HarnessTokens {
                prompt: 1000,
                completion: 100,
                basis: TokenBasis::Shared
            }
        );
    }

    // ── The summary half ──────────────────────────────────────────────────────

    fn report() -> FitnessReport {
        FitnessReport {
            model: "m".to_string(),
            guarded: true,
            unbound: vec![verdict("subject-bound", FitnessBand::Ready)],
            slots: vec![
                crate::fitness::score::SlotVerdict {
                    slot: slot_fixture(SlotKind::Role, "lenient"),
                    harnesses: vec![verdict("ticket", FitnessBand::Ready)],
                    ..slot_verdict("lenient", FitnessBand::Ready)
                },
                crate::fitness::score::SlotVerdict {
                    slot: slot_fixture(SlotKind::Role, "strict"),
                    harnesses: vec![verdict("ticket", FitnessBand::Workable)],
                    ..slot_verdict("strict", FitnessBand::Workable)
                },
            ],
        }
    }

    #[test]
    fn harness_summary_averages_tokens_per_case_and_keeps_the_worst_band() {
        let out = harness_summary(
            &report(),
            &[
                score_like("ticket", 4, 4000, 800),
                score_like("subject-bound", 2, 1000, 500),
            ],
        );

        assert_eq!(
            out.get("ticket"),
            Some(&summary(FitnessBand::Workable, 4, 1000, 200))
        );
        // An unbound harness has no cell to derive from, which is the reason
        // this field exists at all.
        assert_eq!(
            out.get("subject-bound"),
            Some(&summary(FitnessBand::Ready, 2, 500, 250))
        );
    }

    #[test]
    fn harness_summary_records_a_judged_but_unswept_harness_without_dividing_by_zero() {
        let out = harness_summary(&report(), &[]);

        assert_eq!(
            out.get("ticket"),
            Some(&summary(FitnessBand::Workable, 0, 0, 0))
        );
    }

    // ── Bands ─────────────────────────────────────────────────────────────────

    #[test]
    fn band_for_reads_the_entrys_own_per_harness_band_when_it_has_one() {
        let slots_of = slots_by_harness(&[
            slot_binding(SlotKind::Role, "lenient", &["ticket"]),
            slot_binding(SlotKind::Role, "strict", &["ticket"]),
        ]);
        let e = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Workable, 4, 1, 1),
            )])),
            cells: HashMap::from([("role:lenient".to_string(), cell(FitnessBand::Ready))]),
            ..entry()
        };

        assert_eq!(
            band_for("ticket", Some(&e), &slots_of),
            FitnessBand::Workable
        );
    }

    #[test]
    fn band_for_falls_back_to_the_worst_cell_across_the_bound_slots() {
        // Same numbers, two task floors. A permissive slot must not launder a
        // verdict the strict one refused.
        let slots_of = slots_by_harness(&[
            slot_binding(SlotKind::Role, "lenient", &["ticket"]),
            slot_binding(SlotKind::Role, "strict", &["ticket"]),
        ]);
        let e = FitnessIndexEntry {
            cells: HashMap::from([
                ("role:lenient".to_string(), cell(FitnessBand::Ready)),
                ("role:strict".to_string(), cell(FitnessBand::Workable)),
            ]),
            ..entry()
        };

        assert_eq!(
            band_for("ticket", Some(&e), &slots_of),
            FitnessBand::Workable
        );
    }

    #[test]
    fn band_for_is_untested_for_no_report_and_for_an_unbound_harness_on_an_old_entry() {
        let slots_of = slots_by_harness(&[slot_binding(SlotKind::Role, "lenient", &["ticket"])]);
        assert_eq!(band_for("ticket", None, &slots_of), FitnessBand::Untested);
        let e = FitnessIndexEntry {
            cells: HashMap::from([("role:lenient".to_string(), cell(FitnessBand::Ready))]),
            ..entry()
        };
        assert_eq!(
            band_for("subject-bound", Some(&e), &slots_of),
            FitnessBand::Untested
        );
    }

    // ── One model's row ───────────────────────────────────────────────────────

    /// 600 ticket runs and 30 brief runs over the 30-day window: 20/day and
    /// 1/day.
    fn value_fixture() -> (Workload, HashMap<String, Vec<String>>) {
        let workload = workload_from(
            &[observed("ticket", 600), observed("brief", 30)],
            &[harness("ticket", 2), harness("brief", 2)],
            30,
        );
        let slots_of = slots_by_harness(&[
            slot_binding(SlotKind::Role, "worker", &["ticket"]),
            slot_binding(SlotKind::Role, "writer", &["brief"]),
        ]);
        (workload, slots_of)
    }

    fn tested_entry() -> FitnessIndexEntry {
        FitnessIndexEntry {
            harnesses: Some(HashMap::from([
                (
                    "ticket".to_string(),
                    summary(FitnessBand::Ready, 4, 1000, 200),
                ),
                (
                    "brief".to_string(),
                    summary(FitnessBand::Unfit, 4, 500, 1000),
                ),
            ])),
            ..entry()
        }
    }

    fn price() -> ModelPrice {
        ModelPrice {
            in_per_mtok: 1.0,
            out_per_mtok: 4.0,
        }
    }

    #[test]
    fn value_prices_the_measured_day_and_reports_the_shares_it_is_priced_over() {
        let (workload, slots_of) = value_fixture();
        let v = value_of(
            "m",
            Some(&tested_entry()),
            Some(price()),
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        // ticket: 20 runs × (1000 × $1 + 200 × $4) / 1e6 = $0.036
        // brief:   1 run  × (500  × $1 + 1000 × $4) / 1e6 = $0.0045
        close(v.usd_per_day.unwrap(), 0.0405);
        close(v.ready_share, 20.0 / 21.0);
        close(v.shares.unfit, 1.0 / 21.0);
        close(v.shares.sum(), 1.0);
        close(v.cost_coverage, 1.0);
        assert_eq!(v.token_basis, TokenBasis::Model);
    }

    #[test]
    fn value_divides_cost_by_the_runs_it_is_actually_trusted_with() {
        let (workload, slots_of) = value_fixture();
        let v = value_of(
            "m",
            Some(&tested_entry()),
            Some(price()),
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        close(v.usd_per_ready_run.unwrap(), 0.0405 / 20.0);
    }

    #[test]
    fn value_refuses_a_per_ready_run_figure_when_the_model_is_ready_for_nothing() {
        // The number would be a division by a zero the page cannot show, and
        // the honest reading is not "infinitely expensive" but "no answer".
        let (workload, slots_of) = value_fixture();
        let useless = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Unfit, 4, 1000, 200),
            )])),
            ..entry()
        };
        let v = value_of(
            "m",
            Some(&useless),
            Some(price()),
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        assert!(v.usd_per_day.unwrap() > 0.0);
        assert_eq!(v.usd_per_ready_run, None);
    }

    #[test]
    fn value_reports_coverage_below_one_when_part_of_the_day_has_no_measured_tokens() {
        let (workload, slots_of) = value_fixture();
        let partial = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Ready, 4, 1000, 200),
            )])),
            ..entry()
        };
        let v = value_of(
            "m",
            Some(&partial),
            Some(price()),
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        close(v.cost_coverage, 20.0 / 21.0);
        close(v.usd_per_day.unwrap(), 0.036);
    }

    #[test]
    fn value_marks_the_basis_shared_as_soon_as_one_harness_borrows_the_global_budget() {
        let (workload, slots_of) = value_fixture();
        let partial = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Ready, 4, 1000, 200),
            )])),
            ..entry()
        };
        let mut budget = TokenBudget::new();
        budget.insert(
            "brief".to_string(),
            crate::fitness::surface::TokenBudgetEntry {
                prompt: 500,
                completion: 1000,
                at: "x".to_string(),
            },
        );
        let v = value_of(
            "m",
            Some(&partial),
            Some(price()),
            &workload,
            &budget,
            &slots_of,
        );

        close(v.cost_coverage, 1.0);
        assert_eq!(v.token_basis, TokenBasis::Shared);
    }

    #[test]
    fn value_leaves_an_unpriced_model_off_the_cost_axis_rather_than_at_zero() {
        let (workload, slots_of) = value_fixture();
        let v = value_of(
            "m",
            Some(&tested_entry()),
            None,
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        assert_eq!(v.usd_per_day, None);
        assert_eq!(v.usd_per_ready_run, None);
        // The performance half does not depend on a catalog being reachable.
        close(v.ready_share, 20.0 / 21.0);
    }

    #[test]
    fn value_reports_a_never_tested_model_as_untested_across_the_whole_day() {
        let (workload, slots_of) = value_fixture();
        let v = value_of(
            "m",
            None,
            Some(price()),
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        close(v.shares.untested, 1.0);
        close(v.ready_share, 0.0);
        assert_eq!(v.at, None);
    }

    // ── The whole view ────────────────────────────────────────────────────────

    fn cheap_dear_index() -> FitnessIndex {
        let mut index = FitnessIndex::new();
        index.insert(
            "cheap".to_string(),
            FitnessIndexEntry {
                model: "cheap".to_string(),
                cells: HashMap::from([
                    ("role:worker".to_string(), cell(FitnessBand::Ready)),
                    ("role:writer".to_string(), cell(FitnessBand::Unfit)),
                ]),
                harnesses: Some(HashMap::from([
                    (
                        "ticket".to_string(),
                        summary(FitnessBand::Ready, 4, 1000, 200),
                    ),
                    (
                        "brief".to_string(),
                        summary(FitnessBand::Unfit, 4, 500, 1000),
                    ),
                ])),
                ..entry()
            },
        );
        index.insert(
            "dear".to_string(),
            FitnessIndexEntry {
                model: "dear".to_string(),
                cells: HashMap::from([
                    ("role:worker".to_string(), cell(FitnessBand::Ready)),
                    ("role:writer".to_string(), cell(FitnessBand::Ready)),
                ]),
                harnesses: Some(HashMap::from([
                    (
                        "ticket".to_string(),
                        summary(FitnessBand::Ready, 4, 1000, 200),
                    ),
                    (
                        "brief".to_string(),
                        summary(FitnessBand::Ready, 4, 500, 1000),
                    ),
                ])),
                ..entry()
            },
        );
        index
    }

    fn deps_default() -> ValueDeps {
        let registry = vec![harness("ticket", 2), harness("brief", 2)];
        let bindings = vec![
            slot_binding_as(SlotKind::Role, "worker", &["ticket"], "Ticket worker"),
            slot_binding_as(SlotKind::Role, "writer", &["brief"], "Writer"),
        ];
        let index = cheap_dear_index();
        ValueDeps {
            observed: Arc::new(move || {
                let rows = vec![observed("ticket", 600), observed("brief", 30)];
                Box::pin(async move { Ok(rows) })
            }),
            harnesses: Arc::new(move || {
                let r = registry.clone();
                Box::pin(async move { Ok(r) })
            }),
            bindings: Arc::new(move |_h| {
                let b = bindings.clone();
                Box::pin(async move { Ok(b) })
            }),
            index: Arc::new(move || {
                let i = index.clone();
                Box::pin(async move { Ok(i) })
            }),
            budget: Arc::new(|| Box::pin(async { Ok(TokenBudget::new()) })),
            price: Arc::new(|model| {
                Box::pin(async move {
                    Ok(if model == "cheap" {
                        Some(ModelPrice {
                            in_per_mtok: 0.1,
                            out_per_mtok: 0.4,
                        })
                    } else {
                        Some(ModelPrice {
                            in_per_mtok: 10.0,
                            out_per_mtok: 40.0,
                        })
                    })
                })
            }),
            record: Arc::new(|_m| Box::pin(async { Ok(None) })),
            window_days: 30,
        }
    }

    #[tokio::test]
    async fn the_view_ranks_a_slot_by_band_first_and_price_second() {
        let view = value_view(&deps_default()).await.unwrap();
        let worker = view.slots.iter().find(|s| s.key == "role:worker").unwrap();

        let names: Vec<&str> = worker.candidates.iter().map(|c| c.model.as_str()).collect();
        assert_eq!(names, ["cheap", "dear"]);
        assert_eq!(worker.best.as_deref(), Some("cheap"));
        // The slot's own bill, not the whole workload's: ticket only.
        close(
            worker.candidates[0].usd_per_day.unwrap(),
            (20.0 * (1000.0 * 0.1 + 200.0 * 0.4)) / 1e6,
        );
    }

    #[tokio::test]
    async fn the_view_offers_only_the_models_that_clear_the_floor() {
        let view = value_view(&deps_default()).await.unwrap();
        let writer = view.slots.iter().find(|s| s.key == "role:writer").unwrap();

        let names: Vec<&str> = writer.candidates.iter().map(|c| c.model.as_str()).collect();
        assert_eq!(names, ["dear"]);
        assert_eq!(writer.best.as_deref(), Some("dear"));
        close(writer.per_day, 1.0);
    }

    #[tokio::test]
    async fn the_view_does_not_let_the_cheaper_model_win_the_day_it_cannot_carry() {
        // The whole point of the second axis. `cheap` is 100× less per token
        // and covers 95% of the runs; `dear` covers all of them. Neither number
        // alone decides, and the page must carry both.
        let view = value_view(&deps_default()).await.unwrap();
        let cheap = view.models.iter().find(|m| m.model == "cheap").unwrap();
        let dear = view.models.iter().find(|m| m.model == "dear").unwrap();

        assert!(cheap.usd_per_day.unwrap() < dear.usd_per_day.unwrap());
        assert!(cheap.ready_share < dear.ready_share);
        close(dear.ready_share, 1.0);
    }

    #[tokio::test]
    async fn the_view_names_the_harnesses_nothing_has_measured_tokens_for() {
        let mut deps = deps_default();
        let unswept = harness("unswept", 2);
        deps.observed = Arc::new(move || {
            let rows = vec![
                observed("ticket", 30),
                observed("brief", 30),
                observed("unswept", 30),
            ];
            Box::pin(async move { Ok(rows) })
        });
        deps.harnesses = Arc::new(move || {
            let r = vec![harness("ticket", 2), harness("brief", 2), unswept];
            Box::pin(async move { Ok(r) })
        });

        let view = value_view(&deps).await.unwrap();
        assert_eq!(view.unmeasured, vec!["unswept".to_string()]);
    }

    #[tokio::test]
    async fn the_view_turns_the_cost_axis_off_rather_than_drawing_an_unpriced_fleet_at_zero() {
        let mut deps = deps_default();
        deps.price = Arc::new(|_m| Box::pin(async { Ok(None) }));

        let view = value_view(&deps).await.unwrap();

        assert!(!view.priced);
        assert!(view.models.iter().all(|m| m.usd_per_day.is_none()));
        assert!(
            view.slots
                .iter()
                .all(|s| s.candidates.iter().all(|c| c.usd_per_day.is_none()))
        );
    }

    #[tokio::test]
    async fn the_view_renders_on_a_fresh_install_saying_which_basis_it_drew() {
        let mut deps = deps_default();
        deps.observed = Arc::new(|| Box::pin(async { Ok(Vec::new()) }));

        let view = value_view(&deps).await.unwrap();

        assert_eq!(view.workload.basis, WorkloadBasis::Uniform);
        close(view.workload.per_day, 2.0);
        assert_eq!(view.models.len(), 2);
    }

    #[tokio::test]
    async fn the_view_survives_a_telemetry_query_that_throws() {
        // Advisory data behind a page that must still render — the same
        // posture observed.rs takes for the matrix.
        let mut deps = deps_default();
        deps.observed = Arc::new(|| Box::pin(async { Err("telemetry query threw".into()) }));

        let view = value_view(&deps).await.unwrap();
        assert_eq!(view.workload.basis, WorkloadBasis::Uniform);
    }

    #[tokio::test]
    async fn the_view_backfills_an_entry_archived_before_the_index_carried_its_half() {
        // The measurement is real and already paid for — it just lives in the
        // full report. Stranding it behind "re-test this model" bills an admin
        // twice.
        let old = FitnessIndexEntry {
            model: "old".to_string(),
            cells: HashMap::from([("role:worker".to_string(), cell(FitnessBand::Ready))]),
            ..entry()
        };
        let mut deps = deps_default();
        deps.index = Arc::new(move || {
            let mut i = FitnessIndex::new();
            i.insert("old".to_string(), old.clone());
            Box::pin(async move { Ok(i) })
        });
        deps.record = Arc::new(|_m| {
            Box::pin(async {
                Ok(Some(ArchivedRecord {
                    report: FitnessReport {
                        model: "old".to_string(),
                        slots: vec![crate::fitness::score::SlotVerdict {
                            slot: slot_fixture(SlotKind::Role, "worker"),
                            harnesses: vec![verdict("ticket", FitnessBand::Ready)],
                            ..slot_verdict("worker", FitnessBand::Ready)
                        }],
                        unbound: Vec::new(),
                        guarded: true,
                    },
                    harnesses: vec![score_like("ticket", 4, 4000, 800)],
                }))
            })
        });

        let view = value_view(&deps).await.unwrap();

        // Backfilled tokens, and this model's OWN — not the shared budget's.
        let row = &view.models[0];
        assert_eq!(row.token_basis, TokenBasis::Model);
        // `old` is not `cheap`, so it drew the dear 10/40 price.
        close(
            row.usd_per_day.unwrap(),
            (20.0 * (1000.0 * 10.0 + 200.0 * 40.0)) / 1e6,
        );
    }

    #[tokio::test]
    async fn the_view_reads_the_archive_once_per_entry_that_needs_it_and_never_for_one_that_does_not()
     {
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut deps = deps_default();
        let sink = asked.clone();
        deps.record = Arc::new(move |m| {
            sink.lock().unwrap().push(m);
            Box::pin(async { Ok(None) })
        });

        value_view(&deps).await.unwrap();

        // Both fixtures already carry `harnesses`, so nothing is read back.
        assert!(asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_view_falls_back_to_the_cells_when_even_the_archived_report_is_gone() {
        let old = FitnessIndexEntry {
            model: "old".to_string(),
            cells: HashMap::from([("role:worker".to_string(), cell(FitnessBand::Ready))]),
            ..entry()
        };
        let mut deps = deps_default();
        deps.index = Arc::new(move || {
            let mut i = FitnessIndex::new();
            i.insert("old".to_string(), old.clone());
            Box::pin(async move { Ok(i) })
        });

        let view = value_view(&deps).await.unwrap();

        close(view.models[0].ready_share, 20.0 / 21.0);
        assert_eq!(view.models[0].token_basis, TokenBasis::None);
    }

    #[tokio::test]
    async fn the_view_lists_only_models_something_has_tested() {
        // Four hundred gateway ids at "untested, 0% ready" is true and is noise.
        let view = value_view(&deps_default()).await.unwrap();

        let names: Vec<&str> = view.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names, ["cheap", "dear"]);
    }

    // ── A price with nothing to price ─────────────────────────────────────────

    #[test]
    fn a_failed_run_reports_no_figure_rather_than_zero_a_day() {
        // A run that failed every case has a perfectly good $/MTok and not one
        // measured token. "$0 a day" would put it at the cheap end of the chart.
        let workload = workload_from(&[observed("ticket", 600)], &[harness("ticket", 2)], 30);
        let slots_of = slots_by_harness(&[slot_binding(SlotKind::Role, "worker", &["ticket"])]);
        let failed = FitnessIndexEntry {
            harnesses: Some(HashMap::from([(
                "ticket".to_string(),
                summary(FitnessBand::Unfit, 4, 0, 0),
            )])),
            ..entry()
        };
        let v = value_of(
            "m",
            Some(&failed),
            Some(ModelPrice {
                in_per_mtok: 10.0,
                out_per_mtok: 40.0,
            }),
            &workload,
            &TokenBudget::new(),
            &slots_of,
        );

        assert_eq!(v.usd_per_day, None);
        close(v.cost_coverage, 0.0);
        assert_eq!(v.token_basis, TokenBasis::None);
    }

    #[tokio::test]
    async fn a_failed_run_says_the_same_thing_per_slot() {
        let mut index = FitnessIndex::new();
        index.insert(
            "failed".to_string(),
            FitnessIndexEntry {
                model: "failed".to_string(),
                cells: HashMap::from([("role:worker".to_string(), cell(FitnessBand::Ready))]),
                harnesses: Some(HashMap::from([(
                    "ticket".to_string(),
                    summary(FitnessBand::Ready, 4, 0, 0),
                )])),
                ..entry()
            },
        );
        let deps = ValueDeps {
            observed: Arc::new(|| {
                let rows = vec![observed("ticket", 600)];
                Box::pin(async move { Ok(rows) })
            }),
            harnesses: Arc::new(|| {
                let r = vec![harness("ticket", 2)];
                Box::pin(async move { Ok(r) })
            }),
            bindings: Arc::new(|_h| {
                let b = vec![slot_binding(SlotKind::Role, "worker", &["ticket"])];
                Box::pin(async move { Ok(b) })
            }),
            index: Arc::new(move || {
                let i = index.clone();
                Box::pin(async move { Ok(i) })
            }),
            budget: Arc::new(|| Box::pin(async { Ok(TokenBudget::new()) })),
            price: Arc::new(|_m| {
                Box::pin(async {
                    Ok(Some(ModelPrice {
                        in_per_mtok: 10.0,
                        out_per_mtok: 40.0,
                    }))
                })
            }),
            record: Arc::new(|_m| Box::pin(async { Ok(None) })),
            window_days: 30,
        };

        let view = value_view(&deps).await.unwrap();
        assert_eq!(view.slots[0].candidates[0].usd_per_day, None);
    }
}

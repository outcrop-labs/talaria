// Zero-config pricing, ported from ui/src/server/price-oracle.ts: OpenRouter's
// public model catalog (no auth) carries per-token prices for essentially every
// major model. Each cloud endpoint's catalog models — PLUS every model usage
// has actually been attributed to on it — are matched against that catalog by
// normalized name, and the result lands in llm_endpoints.auto_prices, separate
// from user overrides (model_prices), which always win. Local endpoints are
// skipped ($0 by definition).
//
// Three callers share one single-flight, exactly as in TS: the scheduled job
// (registered against the scheduler; the flip is the only thing that arms it),
// the opportunistic `maybe_refresh_auto_prices` kick, and the
// `nudge_auto_prices` cue a cloud usage row without a price fires — wired into
// gateway::usage::record_usage, which had been carrying that debt as a note.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Map as JsonMap, Value};
use sqlx::PgPool;

use crate::scheduler::{JobName, JobSpec};

/// 6h between successful refreshes. The cadence used to live inside
/// `maybeRefreshAutoPrices` and only advanced when someone loaded /models — an
/// instance nobody administered priced its usage against a catalog that never
/// updated. The interval is unchanged; the scheduler owns it now.
pub const PRICE_REFRESH_EVERY_MS: u64 = 6 * 60 * 60 * 1000;
/// Earliest of the registered jobs, and the only one that writes nothing a
/// person sees — safe to run soon after boot so a fresh deploy prices
/// correctly.
pub const PRICE_REFRESH_FIRST_RUN_DELAY_MS: u64 = 60_000;
/// One catalog fetch plus a small write per cloud endpoint.
pub const PRICE_REFRESH_MAX_RUN_MS: u64 = 5 * 60_000;
/// No retry storm when the catalog is unreachable: 10min between attempts
/// after a failure.
const RETRY_INTERVAL_MS: i64 = 10 * 60 * 1000;
/// A price-less cloud usage row may ask again sooner than the 6h cadence (new
/// model, new alias) — 15min between nudges, and the failure backoff still
/// applies.
const NUDGE_INTERVAL_MS: i64 = 15 * 60 * 1000;
/// The catalog fetch is bounded whole — send, status, and body (TS:
/// `AbortSignal.timeout(15_000)`), not just the connect.
const FETCH_TIMEOUT_MS: u64 = 15_000;
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
/// Per-mtok prices are stored rounded to 4 decimals (TS: `Number(x.toFixed(4))`).
const PRICE_SCALE: f64 = 10_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokPrice {
    pub in_per_mtok: f64,
    pub out_per_mtok: f64,
}

/// The jsonb shape the costing join reads (`auto_prices->model->>'in'`,
/// gateway/usage.rs) — the key names are load-bearing, hence the renames.
#[derive(serde::Serialize)]
struct AutoPrice {
    #[serde(rename = "in")]
    in_per_mtok: f64,
    #[serde(rename = "out")]
    out_per_mtok: f64,
}

fn round4(x: f64) -> f64 {
    (x * PRICE_SCALE).round() / PRICE_SCALE
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// "anthropic/claude-opus-4.8" → "anthropic/claude-opus-4-8". Leading "~"
/// (Talaria's alias marker) never appears in public catalogs.
fn normalize(s: &str) -> String {
    s.strip_prefix('~')
        .unwrap_or(s)
        .to_lowercase()
        .replace('.', "-")
}

/// Trailing "-YYYYMMDD" off the tail: a dash then exactly eight ASCII digits.
/// Byte-indexed — a matching ASCII dash is always a char boundary.
fn strip_date_suffix(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 9 && b[b.len() - 9] == b'-' && b[b.len() - 8..].iter().all(|c| c.is_ascii_digit())
    {
        s[..s.len() - 9].to_string()
    } else {
        s.to_string()
    }
}

/// The id shapes a model might wear in a public catalog, most-specific first:
/// as-is → ":variant" stripped → trailing "-YYYYMMDD" date stripped →
/// "-latest" stripped. Each step feeds the next, so combinations occur — but
/// each strip fires at most once (the chain, not a fixpoint).
fn id_candidates(model: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |m: String, out: &mut Vec<String>| {
        if !m.is_empty() && !out.contains(&m) {
            out.push(m);
        }
    };
    let mut cur = model.strip_prefix('~').unwrap_or(model).to_string();
    push(cur.clone(), &mut out);
    if cur.contains(':') {
        cur = cur.split(':').next().unwrap_or(&cur).to_string();
        push(cur.clone(), &mut out);
    }
    let dateless = strip_date_suffix(&cur);
    if dateless != cur {
        cur = dateless;
        push(cur.clone(), &mut out);
    }
    if let Some(unlatest) = cur.strip_suffix("-latest") {
        push(unlatest.to_string(), &mut out);
    }
    out
}

struct SuffixEntry {
    vendor: String,
    price: TokPrice,
}

struct Catalog {
    /// Keyed by the FULL normalized id ("anthropic/claude-opus-4-8") — for
    /// vendor-prefixed model ids (OpenRouter-style endpoints).
    by_id: HashMap<String, TokPrice>,
    /// Keyed by the normalized suffix ("claude-opus-4-8") — for bare ids.
    by_suffix: HashMap<String, Vec<SuffixEntry>>,
}

/// TS `Number(x)` on the pricing fields: strings or numbers. Anything else —
/// missing, objects — is NaN in TS and skipped here. (The one hair TS keeps
/// that this drops: `Number("") === 0`; no catalog ships an empty price
/// string, and skipping is the honest read of one that did.)
fn tok_per_token(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Fold the raw OpenRouter `data` array into the lookup shapes. Per-token
/// prices are scaled to per-mtok here, once. `m.id.split('/')`: the vendor is
/// the FIRST segment, the suffix is the rest (or the whole id when bare).
fn build_catalog(models: &[Value]) -> Catalog {
    let mut by_id = HashMap::new();
    let mut by_suffix: HashMap<String, Vec<SuffixEntry>> = HashMap::new();
    for m in models {
        let Some(id) = m.get("id").and_then(Value::as_str) else {
            continue;
        };
        let pricing = m.get("pricing");
        let (Some(in_tok), Some(out_tok)) = (
            tok_per_token(pricing.and_then(|p| p.get("prompt"))),
            tok_per_token(pricing.and_then(|p| p.get("completion"))),
        ) else {
            continue;
        };
        let price = TokPrice {
            in_per_mtok: in_tok * 1e6,
            out_per_mtok: out_tok * 1e6,
        };
        by_id.insert(normalize(id), price);
        let (vendor, rest) = match id.split_once('/') {
            Some((v, r)) => (v, r),
            None => (id, ""),
        };
        let suffix = if rest.is_empty() {
            normalize(vendor)
        } else {
            normalize(rest)
        };
        by_suffix.entry(suffix).or_default().push(SuffixEntry {
            vendor: normalize(vendor),
            price,
        });
    }
    Catalog { by_id, by_suffix }
}

fn suffix_match(catalog: &Catalog, vendor: &str, model: &str) -> Option<TokPrice> {
    let candidates = catalog.by_suffix.get(&normalize(model))?;
    let vendor = normalize(vendor);
    if let Some(hit) = candidates.iter().find(|c| c.vendor == vendor) {
        return Some(hit.price);
    }
    let first = candidates[0].price;
    let unambiguous = candidates.iter().all(|c| c.price == first);
    unambiguous.then_some(first)
}

/// Pick a price for a model on a provider (TS's `match`). Vendor-prefixed ids
/// match the full catalog id; bare ids match by suffix (exact vendor wins, else
/// only an unambiguous candidate). Alias shapes (":free" variants, "-latest",
/// trailing dates, "~" prefixes) fall back through id_candidates. None = no
/// match — stays unpriced, never guessed.
fn price_for(catalog: &Catalog, provider: &str, model: &str) -> Option<TokPrice> {
    for id in id_candidates(model) {
        if id.contains('/') {
            if let Some(hit) = catalog.by_id.get(&normalize(&id)) {
                return Some(*hit);
            }
            // A vendor-prefixed id can still suffix-match when the catalog
            // files the model under a different vendor path (rare, but
            // aliases do this).
            if let Some((vendor, rest)) = id.split_once('/')
                && let Some(hit) = suffix_match(catalog, vendor, rest)
            {
                return Some(hit);
            }
            continue;
        }
        if let Some(hit) = suffix_match(catalog, provider, &id) {
            return Some(hit);
        }
    }
    None
}

/// Fetch and fold OpenRouter's public catalog. Errors are strings because the
/// scheduler's contract carries text (the job's error line).
async fn fetch_catalog() -> Result<Catalog, String> {
    let fetch = async {
        let resp = crate::gateway::provider::http()
            .get(OPENROUTER_MODELS_URL)
            .send()
            .await
            .map_err(|e| format!("openrouter catalog fetch failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("openrouter {}", resp.status().as_u16()));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("openrouter catalog was not json: {e}"))
    };
    let body = tokio::time::timeout(Duration::from_millis(FETCH_TIMEOUT_MS), fetch)
        .await
        .map_err(|_| "openrouter catalog fetch timed out".to_string())??;
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(build_catalog(&data))
}

#[derive(Debug, Default, PartialEq)]
pub struct RefreshCounts {
    pub priced: usize,
    pub endpoints: usize,
}

/// refreshAutoPrices — refresh auto_prices for every cloud endpoint. The
/// second model set (usage-attributed) is what keeps costing honest: tier
/// routing and aliases send usage to models nobody registered, and those rows
/// must price too. Keys are the EXACT strings usage carries, so the costing
/// join hits directly. Every cloud endpoint gets its write, priced or not —
/// TS updates unconditionally, and an emptied catalog should clear stale
/// prices.
async fn refresh_auto_prices(pg: &PgPool) -> Result<RefreshCounts, String> {
    let catalog = fetch_catalog().await?;
    let endpoints: Vec<(String, String, String, Value)> = sqlx::query_as(
        "select id::text, name, provider, models from llm_endpoints where class = 'cloud'",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("cloud endpoint read failed: {e}"))?;
    let seen: Vec<(String, String)> = sqlx::query_as(
        "select distinct endpoint, llm_model from usage_events \
         where endpoint is not null and llm_model is not null",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("usage read failed: {e}"))?;

    let mut counts = RefreshCounts::default();
    for (id, name, provider, models) in endpoints {
        let mut all: HashSet<String> = models
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for (endpoint, model) in &seen {
            if endpoint == &name {
                all.insert(model.clone());
            }
        }
        let mut auto = JsonMap::new();
        for m in all {
            if let Some(hit) = price_for(&catalog, &provider, &m) {
                let value = serde_json::to_value(AutoPrice {
                    in_per_mtok: round4(hit.in_per_mtok),
                    out_per_mtok: round4(hit.out_per_mtok),
                })
                .expect("an f64 pair serializes");
                auto.insert(m, value);
                counts.priced += 1;
            }
        }
        sqlx::query(
            "update llm_endpoints set auto_prices = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(Value::Object(auto))
        .bind(&id)
        .execute(pg)
        .await
        .map_err(|e| format!("auto_prices write failed: {e}"))?;
        counts.endpoints += 1;
    }
    Ok(counts)
}

struct Stamps {
    last_refresh: i64,
    last_attempt: i64,
}

static STAMPS: LazyLock<Mutex<Stamps>> = LazyLock::new(|| {
    Mutex::new(Stamps {
        last_refresh: 0,
        last_attempt: 0,
    })
});

/// The single-flight. Held across the whole pass — that is its job: a caller
/// arriving mid-pass waits, then re-checks its throttle instead of fetching
/// again, which is the Rust shape of TS's shared in-flight promise. The one
/// divergence: TS's scheduled job riding a concurrent kick shares that
/// promise's result; here it waits and refreshes again — at most one
/// redundant catalog fetch per 6h window, never two at once.
static FLIGHT: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn stamps() -> std::sync::MutexGuard<'static, Stamps> {
    STAMPS.lock().unwrap_or_else(|p| p.into_inner())
}

/// The throttle both fire-and-forget callers gate on: the last SUCCESS must be
/// older than this caller's interval, and the last ATTEMPT older than the
/// failure backoff. Pure, so the gate is testable without a clock.
fn gate_open(s: &Stamps, now: i64, min_gap_ms: i64) -> bool {
    now - s.last_refresh >= min_gap_ms && now - s.last_attempt >= RETRY_INTERVAL_MS
}

/// refreshOnce — the single-flight every caller goes through, so the catalog
/// is never fetched twice at once. `min_gap_ms`: Some for the fire-and-forget
/// callers (checked INSIDE the flight lock, so a caller that queued behind a
/// finished pass doesn't fetch again); None for the scheduled job, whose
/// cadence is the scheduler's, not this gate's. Returns Err when the refresh
/// failed — the scheduler records that; the two fire-and-forget callers below
/// swallow on purpose, because for them the next pass is the retry.
async fn refresh_once(
    pg: &PgPool,
    min_gap_ms: Option<i64>,
) -> Option<Result<RefreshCounts, String>> {
    let _flight = FLIGHT.lock().await;
    if min_gap_ms.is_some_and(|gap| !gate_open(&stamps(), now_ms(), gap)) {
        return None;
    }
    stamps().last_attempt = now_ms();
    let result = refresh_auto_prices(pg).await;
    if result.is_ok() {
        stamps().last_refresh = now_ms();
    }
    Some(result)
}

/// maybeRefreshAutoPrices — fire-and-forget: at most every 6h after a success,
/// and no retry storm when offline (10min backoff between failed attempts).
/// Call from hot read paths. Offline/unreachable → prices stay as they were.
pub fn maybe_refresh_auto_prices(pg: &PgPool) {
    let pg = pg.clone();
    tokio::spawn(async move {
        let _ = refresh_once(&pg, Some(PRICE_REFRESH_EVERY_MS as i64)).await;
    });
}

/// nudgeAutoPrices — usage just landed on a cloud model with NO price:
/// refresh sooner than the 6h cadence (new model, new alias), but never
/// storm.
pub fn nudge_auto_prices(pg: &PgPool) {
    let pg = pg.clone();
    tokio::spawn(async move {
        let _ = refresh_once(&pg, Some(NUDGE_INTERVAL_MS)).await;
    });
}

/// The routes' `void refreshAutoPrices()` — an IMMEDIATE ungated refresh
/// (TS's refreshAutoPrices has no gate; admin edits to the endpoints table
/// want fresh auto-prices now, not at the next 6h tick). Fire-and-forget,
/// errors swallowed like the other kicks.
pub fn kick_auto_prices(pg: &PgPool) {
    let pg = pg.clone();
    tokio::spawn(async move {
        let _ = refresh_once(&pg, None).await;
    });
}

pub struct PriceRefreshDeps {
    pub pg: PgPool,
}

pub fn price_refresh_job_spec(deps: Arc<PriceRefreshDeps>) -> JobSpec {
    JobSpec {
        name: JobName::PriceRefresh,
        every_ms: PRICE_REFRESH_EVERY_MS,
        first_run_delay_ms: Some(PRICE_REFRESH_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(PRICE_REFRESH_MAX_RUN_MS),
        // NOT `per_instance`: the inputs are the shared tables (llm_endpoints,
        // usage_events), so the fleet does one pass per interval — same shape
        // and same reasoning as run-reclaim's comment.
        per_instance: false,
        run: Arc::new(move || {
            let pg = deps.pg.clone();
            Box::pin(async move {
                match refresh_once(&pg, None).await {
                    Some(Ok(c)) => Ok(Some(format!(
                        "{} model price(s) across {} cloud endpoint(s)",
                        c.priced, c.endpoints
                    ))),
                    Some(Err(e)) => Err(e),
                    // The job passes no throttle gate, so this arm is
                    // unreachable — written as a no-op rather than a panic
                    // only because the type demands it.
                    None => Ok(None),
                }
            })
        }),
    }
}

pub fn register_price_refresh_job(deps: Arc<PriceRefreshDeps>) {
    crate::scheduler::register_job(price_refresh_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, p: &str, c: &str) -> Value {
        json!({ "id": id, "pricing": { "prompt": p, "completion": c } })
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn normalization_lowercases_strips_the_alias_marker_and_dots() {
        assert_eq!(
            normalize("anthropic/claude-opus-4.8"),
            "anthropic/claude-opus-4-8"
        );
        assert_eq!(normalize("~GPT.5.Mini"), "gpt-5-mini");
        assert_eq!(normalize("no-marker"), "no-marker");
    }

    #[test]
    fn id_candidates_walk_the_alias_shapes_most_specific_first() {
        assert_eq!(id_candidates("m"), ["m"]);
        assert_eq!(id_candidates("~m"), ["m"]);
        // ":variant" keeps only what precedes the FIRST colon.
        assert_eq!(
            id_candidates("openai/gpt-5:free"),
            ["openai/gpt-5:free", "openai/gpt-5"]
        );
        assert_eq!(id_candidates("gpt-5-20260101"), ["gpt-5-20260101", "gpt-5"]);
        // Each strip fires at most once: -latest exposes the dated id, and
        // the date strip has already had its turn.
        assert_eq!(
            id_candidates("gpt-5-20260101-latest"),
            ["gpt-5-20260101-latest", "gpt-5-20260101"]
        );
        assert_eq!(
            id_candidates("gpt-5:mini-20260101"),
            ["gpt-5:mini-20260101", "gpt-5"]
        );
        // Duplicates never enter the list twice.
        assert_eq!(id_candidates("a:b"), ["a:b", "a"]);
    }

    #[test]
    fn a_full_vendor_prefixed_id_matches_by_id_scaled_per_mtok() {
        let c = build_catalog(&[entry("anthropic/claude-opus-4.8", "0.0000015", "0.0000075")]);
        let hit = price_for(&c, "anthropic", "anthropic/claude-opus-4.8").unwrap();
        assert!(near(hit.in_per_mtok, 1.5));
        assert!(near(hit.out_per_mtok, 7.5));
    }

    #[test]
    fn bare_ids_match_by_suffix_exact_vendor_then_unambiguous_only() {
        let c = build_catalog(&[
            entry("openai/gpt-5", "1", "2"),
            entry("microsoft/gpt-5", "3", "4"),
        ]);
        // Exact vendor wins even when the suffix is ambiguous.
        let hit = price_for(&c, "openai", "gpt-5").unwrap();
        assert!(near(hit.in_per_mtok, 1e6) && near(hit.out_per_mtok, 2e6));
        // Unknown vendor + disagreeing candidates → no match, never guessed.
        assert!(price_for(&c, "acme", "gpt-5").is_none());
        // Unknown vendor + candidates that agree → the shared price.
        let c = build_catalog(&[
            entry("openai/gpt-6", "5", "6"),
            entry("microsoft/gpt-6", "5", "6"),
        ]);
        let hit = price_for(&c, "acme", "gpt-6").unwrap();
        assert!(near(hit.in_per_mtok, 5e6) && near(hit.out_per_mtok, 6e6));
    }

    #[test]
    fn a_vendor_prefixed_id_falls_back_to_suffix_under_another_vendor_path() {
        let c = build_catalog(&[entry("anthropic/claude-opus-4.8", "1", "2")]);
        // byId misses ("acme/…" is not the catalog's path); the suffix is
        // unambiguous, so it still matches.
        let hit = price_for(&c, "acme", "acme/claude-opus-4.8").unwrap();
        assert!(near(hit.in_per_mtok, 1e6));
    }

    #[test]
    fn alias_shapes_fall_back_through_id_candidates() {
        let c = build_catalog(&[entry("openai/gpt-5", "1", "2")]);
        let hit = price_for(&c, "openai", "~gpt-5-latest").unwrap();
        assert!(near(hit.in_per_mtok, 1e6));
        // A dated alias reaches the bare id the same way.
        let hit = price_for(&c, "openai", "gpt-5-20260101").unwrap();
        assert!(near(hit.in_per_mtok, 1e6));
    }

    #[test]
    fn prices_without_numbers_are_skipped() {
        let c = build_catalog(&[
            entry("weird/model-a", "not-a-number", "2"),
            json!({ "id": "weird/model-b" }),
        ]);
        assert!(price_for(&c, "weird", "weird/model-a").is_none());
        assert!(price_for(&c, "weird", "weird/model-b").is_none());
    }

    #[test]
    fn catalog_prices_are_jsonb_keyed_in_and_out() {
        // The costing join reads ->>'in' / ->>'out' — pin the wire shape.
        let value = serde_json::to_value(AutoPrice {
            in_per_mtok: 1.5,
            out_per_mtok: 7.5,
        })
        .unwrap();
        assert_eq!(value, json!({ "in": 1.5, "out": 7.5 }));
    }

    #[test]
    fn prices_round_to_four_decimals_like_to_fixed() {
        assert!(near(round4(1.23456), 1.2346));
        assert!(near(round4(3.0), 3.0));
        assert!(near(round4(0.0000015 * 1e6), 1.5));
    }

    #[test]
    fn the_throttle_gate_needs_a_cold_success_and_a_cold_attempt() {
        let hour = 60 * 60 * 1000;
        let now = 7 * hour;
        let s = Stamps {
            last_refresh: 0,
            last_attempt: 0,
        };
        // Cold boot: open.
        assert!(gate_open(&s, now, 6 * hour));
        // Last success inside the caller's interval: closed.
        let s = Stamps {
            last_refresh: now - 2 * hour,
            last_attempt: 0,
        };
        assert!(!gate_open(&s, now, 6 * hour));
        // Fresh failure backoff — attempted 5min ago, before the 10min line:
        // closed even though no success ever landed.
        let s = Stamps {
            last_refresh: 0,
            last_attempt: now - 5 * 60 * 1000,
        };
        assert!(!gate_open(&s, now, 6 * hour));
    }

    #[tokio::test]
    async fn the_job_spec_carries_the_declared_timings() {
        let deps = Arc::new(PriceRefreshDeps {
            // Lazy pool: never connected — the spec test never runs the job.
            // (Pool construction itself wants a Tokio context; hence async.)
            pg: sqlx::PgPool::connect_lazy("postgresql://spec@127.0.0.1:1/spec")
                .expect("a lazy pool builds without connecting"),
        });
        let spec = price_refresh_job_spec(deps);
        assert_eq!(spec.name.as_str(), "price-refresh");
        assert_eq!(spec.every_ms, 6 * 60 * 60 * 1000);
        assert_eq!(spec.first_run_delay_ms, Some(60_000));
        assert_eq!(spec.max_run_ms, Some(5 * 60_000));
        assert!(
            !spec.per_instance,
            "the catalog and the tables it reads are fleet-shared; it takes the lease"
        );
    }
}

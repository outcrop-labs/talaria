mod spend;
mod spend_fetch;
pub mod unpriced;

// Zero-config pricing: OpenRouter's
// public model catalog (no auth) carries per-token prices for essentially every
// major model. Each cloud endpoint's catalog models — PLUS every model usage
// has actually been attributed to on it — are matched against that catalog by
// normalized name, and the result lands in llm_endpoints.auto_prices, separate
// from user overrides (model_prices), which always win. Each hit is also
// upserted into provider_prices as a model-level published rate (variant '',
// source 'openrouter.catalog') — direct providers publish no prices of their
// own, so that row is what their usage is derived from. Local endpoints are
// skipped ($0 by definition).
//
// Three callers share one single-flight: the scheduled job
// (registered against the scheduler; the boot wiring arms it),
// the opportunistic `maybe_refresh_auto_prices` kick, and the
// `nudge_auto_prices` cue a cloud usage row without a price fires — wired into
// gateway::usage::record_relay_usage (its `nudge_if_unpriced`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Map as JsonMap, Value};
use sqlx::PgPool;

use talaria_agent_auth::now_ms;
use talaria_scheduler::{JobName, JobSpec};

/// 6h between successful refreshes, owned by the scheduler so an instance
/// nobody administers still re-prices — an on-load-only cadence never advances
/// on a quiet install.
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
/// The catalog fetch is bounded whole — send, status, and body,
/// not just the connect.
const FETCH_TIMEOUT_MS: u64 = 15_000;
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
/// Per-mtok prices are stored rounded to 4 decimals.
const PRICE_SCALE: f64 = 10_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokPrice {
    pub in_per_mtok: f64,
    pub out_per_mtok: f64,
}

/// One catalog entry's price: the input/output pair plus the catalog's own
/// cache rates (`input_cache_write` / `input_cache_read`), when it publishes
/// them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CatalogPrice {
    in_per_mtok: f64,
    out_per_mtok: f64,
    cache_write_per_mtok: Option<f64>,
    cache_read_per_mtok: Option<f64>,
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

/// "anthropic/claude-opus-4.8" → "anthropic/claude-opus-4-8". Leading "~"
/// (Talaria's alias marker) never appears in public catalogs.
fn normalize(s: &str) -> String {
    s.strip_prefix('~')
        .unwrap_or(s)
        .to_lowercase()
        .replace('.', "-")
}

/// A snapshot date off the tail, in the three shapes providers stamp ids
/// with: Anthropic's "-YYYYMMDD" (`claude-sonnet-4-5-20250929`), OpenAI's
/// "-YYYY-MM-DD" (`gpt-4.1-2025-04-14`), and the "-MMDD" of xAI and older
/// OpenAI ids (`grok-4-0709`, `gpt-4-0613`) — the last only when it reads as
/// a real month and day, so a "-2507" (Qwen's YYMM) or a "-1234" is left
/// alone. Byte-indexed — a matching ASCII dash is always a char boundary.
fn strip_date_suffix(s: &str) -> String {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let two = |i: usize| (b[i] - b'0') * 10 + (b[i + 1] - b'0');
    let n = b.len();
    // -YYYY-MM-DD: 11 bytes.
    if n >= 11
        && b[n - 11] == b'-'
        && digits(n - 10..n - 6)
        && b[n - 6] == b'-'
        && digits(n - 5..n - 3)
        && b[n - 3] == b'-'
        && digits(n - 2..n)
    {
        return s[..n - 11].to_string();
    }
    // -YYYYMMDD: 9 bytes.
    if n >= 9 && b[n - 9] == b'-' && digits(n - 8..n) {
        return s[..n - 9].to_string();
    }
    // -MMDD: 5 bytes, a plausible month and day, with something before the
    // dash to keep.
    if n >= 6 && b[n - 5] == b'-' && digits(n - 4..n) {
        let (month, day) = (two(n - 4), two(n - 2));
        if (1..=12).contains(&month) && (1..=31).contains(&day) {
            return s[..n - 5].to_string();
        }
    }
    s.to_string()
}

/// The id shapes a model might wear in a public catalog, most-specific first:
/// as-is → ":variant" stripped → a trailing snapshot date stripped (see
/// `strip_date_suffix`) → "-latest" stripped → for a multi-segment path
/// (Fireworks' `accounts/fireworks/models/<id>`), its last segment. Each step
/// feeds the next, so combinations occur — but each strip fires at most once
/// (the chain, not a fixpoint).
pub(crate) fn id_candidates(model: &str) -> Vec<String> {
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
    if cur.matches('/').count() >= 2
        && let Some(last) = cur.rsplit('/').next()
    {
        push(last.to_string(), &mut out);
    }
    out
}

struct SuffixEntry {
    vendor: String,
    price: CatalogPrice,
}

struct Catalog {
    by_id: HashMap<String, CatalogPrice>,
    by_suffix: HashMap<String, Vec<SuffixEntry>>,
    /// How many priced models the catalog carried — a miss says how big the
    /// list it was not in was.
    size: usize,
    /// Normalized ids the catalog lists with a variable (negative) price.
    variable: HashSet<String>,
}

/// The pricing fields arrive as strings or numbers. Anything else —
/// missing, objects, an empty string — is skipped. (No catalog ships an empty
/// price string, and skipping is the honest read of one that did.)
///
/// A PRICE IS A FINITE NUMBER, ZERO OR MORE. OpenRouter publishes `"-1"` as
/// its marker for "variable" — its routers (`openrouter/auto`,
/// `openrouter/fusion`, `nvidia/switchyard` …) bill whatever model each call
/// was routed to — and a parse that kept it priced those calls at -$1 per
/// token, which sank every USD budget below zero. Anything negative or
/// non-finite is no price; `raw_per_token` says which entries were refused.
fn tok_per_token(v: Option<&Value>) -> Option<f64> {
    raw_per_token(v).filter(|n| n.is_finite() && *n >= 0.0)
}

/// The number as published, sign and all — only for telling a variable
/// price (negative) apart from a missing one.
fn raw_per_token(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Fold the raw OpenRouter `data` array into the lookup shapes. Per-token
/// prices are scaled to per-mtok here, once. The vendor is
/// the FIRST segment of the id, the suffix is the rest (or the whole id when
/// bare).
fn build_catalog(models: &[Value]) -> Catalog {
    let mut by_id = HashMap::new();
    let mut by_suffix: HashMap<String, Vec<SuffixEntry>> = HashMap::new();
    let mut variable = HashSet::new();
    for m in models {
        let Some(id) = m.get("id").and_then(Value::as_str) else {
            continue;
        };
        let pricing = m.get("pricing");
        // A negative prompt or completion price is the catalog saying
        // "variable": no entry, and remembered so the miss can say why.
        let published_negative = ["prompt", "completion"]
            .iter()
            .any(|k| raw_per_token(pricing.and_then(|p| p.get(*k))).is_some_and(|n| n < 0.0));
        if published_negative {
            variable.insert(normalize(id));
            continue;
        }
        let (Some(in_tok), Some(out_tok)) = (
            tok_per_token(pricing.and_then(|p| p.get("prompt"))),
            tok_per_token(pricing.and_then(|p| p.get("completion"))),
        ) else {
            continue;
        };
        let price = CatalogPrice {
            in_per_mtok: in_tok * 1e6,
            out_per_mtok: out_tok * 1e6,
            cache_write_per_mtok: tok_per_token(pricing.and_then(|p| p.get("input_cache_write")))
                .map(|p| p * 1e6),
            cache_read_per_mtok: tok_per_token(pricing.and_then(|p| p.get("input_cache_read")))
                .map(|p| p * 1e6),
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
    let size = by_id.len();
    Catalog {
        by_id,
        by_suffix,
        size,
        variable,
    }
}

/// The catalog id a model's lookup would have taken, when the catalog lists
/// it with a VARIABLE price — only vendor-prefixed (exact catalog ids), never
/// a bare suffix: a custom endpoint's `auto` is not OpenRouter's router.
fn variable_entry(catalog: &Catalog, model: &str) -> Option<String> {
    id_candidates(model)
        .into_iter()
        .find(|id| id.contains('/') && catalog.variable.contains(&normalize(id)))
}

/// The sentence a variable-priced model records instead of a price.
fn variable_detail(catalog_id: &str) -> String {
    format!(
        "OpenRouter's catalog publishes a variable price for {catalog_id} (\"-1\": each call is billed \
         as the model it was routed to), so no per-token price applies. Calls that carry the \
         provider's own charge are priced by it."
    )
}

fn suffix_match(catalog: &Catalog, vendor: &str, model: &str) -> Option<CatalogPrice> {
    let candidates = catalog.by_suffix.get(&normalize(model))?;
    let vendor = normalize(vendor);
    if let Some(hit) = candidates.iter().find(|c| c.vendor == vendor) {
        return Some(hit.price);
    }
    let first = candidates[0].price;
    let unambiguous = candidates.iter().all(|c| c.price == first);
    unambiguous.then_some(first)
}

/// Pick a price for a model on a provider. Vendor-prefixed ids
/// match the full catalog id; bare ids match by suffix (exact vendor wins, else
/// only an unambiguous candidate). Alias shapes (":free" variants, "-latest",
/// trailing dates, "~" prefixes) fall back through id_candidates. None = no
/// match — stays unpriced, never guessed.
fn price_for(catalog: &Catalog, provider: &str, model: &str) -> Option<CatalogPrice> {
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

/// An error and every cause under it, joined — reqwest's own Display stops at
/// "error sending request for url (…)", and the reason a person can act on
/// (a DNS failure, a refused connection, a proxy, a TLS failure) is in the
/// source chain below it.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(cause) = cur {
        let text = cause.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        cur = cause.source();
    }
    out
}

async fn fetch_catalog() -> Result<Catalog, String> {
    let fetch = async {
        let resp = talaria_gateway::provider::http()
            .get(OPENROUTER_MODELS_URL)
            .send()
            .await
            .map_err(|e| format!("openrouter catalog fetch failed: {}", error_chain(&e)))?;
        if !resp.status().is_success() {
            // The source's own words, not ours: the first line of what it
            // answered, beside the status.
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let said: String = body.trim().chars().take(200).collect();
            return Err(if said.is_empty() {
                format!("openrouter answered {status}")
            } else {
                format!("openrouter answered {status}: {said}")
            });
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
    /// Model-level published rows upserted into provider_prices this pass.
    pub catalog_priced: usize,
    /// provider_prices upserts that failed this pass — surfaced, never
    /// swallowed: a catalog hit the store refused is a pricing gap.
    pub upsert_failed: usize,
    /// The first upsert error, for the warn the job logs.
    pub first_upsert_error: Option<String>,
    /// Models the catalog had no entry for — each one's `price_lookups` row
    /// says which ids were tried.
    pub no_match: usize,
}

/// The model-level published price a catalog hit becomes: variant '' (the
/// rate the spend view matches when no serving variant is known), rounded
/// like every stored rate, and sourced from the public catalog. Direct
/// providers publish no prices of their own — this row is what their usage
/// is derived from.
fn catalog_price_row(model: &str, hit: CatalogPrice) -> spend::PublishedPrice {
    spend::PublishedPrice {
        model: model.to_string(),
        variant: String::new(),
        price_in_per_mtok: Some(round4(hit.in_per_mtok)),
        price_out_per_mtok: Some(round4(hit.out_per_mtok)),
        price_cache_write_per_mtok: hit.cache_write_per_mtok.map(round4),
        price_cache_read_per_mtok: hit.cache_read_per_mtok.map(round4),
        source: "openrouter.catalog",
    }
}

/// The source every catalog lookup records under.
pub(crate) const CATALOG_SOURCE: &str = "openrouter.catalog";

/// The sentence a catalog miss records: how big the list was and every id
/// shape that was tried, so a person can see the gap rather than guess it.
fn no_match_detail(model: &str, catalog_size: usize) -> String {
    let tried = id_candidates(model);
    let also: Vec<&str> = tried
        .iter()
        .map(String::as_str)
        .filter(|t| *t != model)
        .collect();
    let mut out = format!(
        "OpenRouter's public catalog ({catalog_size} priced models) has no entry for {model}"
    );
    if !also.is_empty() {
        out.push_str(&format!(" (also tried {})", also.join(", ")));
    }
    out
}

/// One lookup's conclusion for an endpoint's model, upserted so the unpriced
/// list can say why a model has no price and when that was last checked.
async fn record_lookup(
    pg: &PgPool,
    endpoint_id: &str,
    model: &str,
    outcome: &str,
    detail: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into price_lookups (endpoint_id, model, outcome, detail, source, checked_at) \
         values ($1::uuid, $2, $3, $4, $5, now()) \
         on conflict (endpoint_id, model) do update set \
           outcome = excluded.outcome, detail = excluded.detail, \
           source = excluded.source, checked_at = now()",
    )
    .bind(endpoint_id)
    .bind(model)
    .bind(outcome)
    .bind(detail)
    .bind(CATALOG_SOURCE)
    .execute(pg)
    .await
    .map(|_| ())
}

/// The cloud endpoints and the models to price on each: the endpoint's own
/// catalog list plus every model usage has been booked under on it. `only`
/// narrows the pass to one (endpoint name, model) — the Retry button's.
async fn models_to_price(
    pg: &PgPool,
    only: Option<(&str, &str)>,
) -> Result<Vec<(String, String, String, Vec<String>)>, String> {
    let endpoints: Vec<(String, String, String, Value)> = sqlx::query_as(
        "select id::text, name, provider, models from llm_endpoints \
         where class = 'cloud' and ($1::text is null or name = $1)",
    )
    .bind(only.map(|o| o.0))
    .fetch_all(pg)
    .await
    .map_err(|e| format!("cloud endpoint read failed: {e}"))?;
    let seen: Vec<(String, String)> = if only.is_some() {
        Vec::new()
    } else {
        sqlx::query_as(
            "select distinct endpoint, llm_model from usage_events \
             where endpoint is not null and llm_model is not null",
        )
        .fetch_all(pg)
        .await
        .map_err(|e| format!("usage read failed: {e}"))?
    };
    Ok(endpoints
        .into_iter()
        .map(|(id, name, provider, models)| {
            let mut all: Vec<String> = match only {
                Some((_, model)) => vec![model.to_string()],
                None => {
                    let mut set: HashSet<String> = models
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
                            set.insert(model.clone());
                        }
                    }
                    set.into_iter().collect()
                }
            };
            all.sort();
            (id, name, provider, all)
        })
        .collect())
}

/// When the catalog could not be read, say so on every model that has no
/// price to fall back on — quoting the failure — and leave the ones that are
/// priced alone (a published price from an earlier pass still applies).
async fn record_source_failure(
    pg: &PgPool,
    targets: &[(String, String, String, Vec<String>)],
    error: &str,
) {
    for (id, _, _, models) in targets {
        for m in models {
            let priced: Option<(i32,)> = sqlx::query_as(
                "select 1 from provider_prices where endpoint_id = $1::uuid and model = $2 \
                 and price_in_per_mtok is not null and price_out_per_mtok is not null limit 1",
            )
            .bind(id)
            .bind(m)
            .fetch_optional(pg)
            .await
            .unwrap_or(None);
            if priced.is_none() {
                let _ = record_lookup(pg, id, m, "source_failed", Some(error)).await;
            }
        }
    }
}

async fn refresh_auto_prices(
    pg: &PgPool,
    only: Option<(&str, &str)>,
) -> Result<RefreshCounts, String> {
    let targets = models_to_price(pg, only).await?;
    let catalog = match fetch_catalog().await {
        Ok(c) => c,
        Err(e) => {
            record_source_failure(pg, &targets, &e).await;
            return Err(e);
        }
    };

    let mut counts = RefreshCounts::default();
    for (id, _name, provider, models) in targets {
        let mut auto = JsonMap::new();
        for m in models {
            if let Some(hit) = price_for(&catalog, &provider, &m) {
                let value = serde_json::to_value(AutoPrice {
                    in_per_mtok: round4(hit.in_per_mtok),
                    out_per_mtok: round4(hit.out_per_mtok),
                })
                .expect("an f64 pair serializes");
                auto.insert(m.clone(), value);
                counts.priced += 1;
                // The same hit is also a published model-level price for
                // this endpoint.
                let price = catalog_price_row(&m, hit);
                match spend_fetch::upsert_price(pg, &id, &provider, &price).await {
                    Ok(()) => {
                        counts.catalog_priced += 1;
                        let _ = record_lookup(pg, &id, &m, "priced", None).await;
                    }
                    Err(e) => {
                        counts.upsert_failed += 1;
                        let _ = record_lookup(
                            pg,
                            &id,
                            &m,
                            "source_failed",
                            Some(&format!(
                                "the catalog priced it, and storing the price failed: {e}"
                            )),
                        )
                        .await;
                        if counts.first_upsert_error.is_none() {
                            counts.first_upsert_error = Some(e);
                        }
                    }
                }
            } else if let Some(entry) = variable_entry(&catalog, &m) {
                // No price to store: drop any earlier pass's row for this
                // model (one written before negatives were refused), and say
                // why it stays unpriced.
                let _ = sqlx::query(
                    "delete from provider_prices where endpoint_id = $1::uuid and model = $2 \
                     and variant = '' and source = $3",
                )
                .bind(&id)
                .bind(&m)
                .bind(CATALOG_SOURCE)
                .execute(pg)
                .await;
                counts.no_match += 1;
                let detail = variable_detail(&entry);
                let _ = record_lookup(pg, &id, &m, "variable", Some(&detail)).await;
            } else {
                counts.no_match += 1;
                let detail = no_match_detail(&m, catalog.size);
                let _ = record_lookup(pg, &id, &m, "no_match", Some(&detail)).await;
            }
        }
        // A full pass replaces the endpoint's fill map (a model that left the
        // catalog leaves it); a targeted retry merges its one entry in.
        let sql = if only.is_some() {
            "update llm_endpoints set auto_prices = coalesce(auto_prices, '{}'::jsonb) || $1, \
             updated_at = now() where id = $2::uuid"
        } else {
            "update llm_endpoints set auto_prices = $1, updated_at = now() where id = $2::uuid"
        };
        sqlx::query(sql)
            .bind(Value::Object(auto))
            .bind(&id)
            .execute(pg)
            .await
            .map_err(|e| format!("auto_prices write failed: {e}"))?;
        counts.endpoints += 1;
    }
    // auto_prices is a cached column of the endpoints serve window — the
    // next completion must be able to see the fill.
    talaria_gateway::registry::invalidate_endpoints_cache();
    Ok(counts)
}

/// THE RETRY: look one endpoint's model up now, awaited, outside the
/// scheduled pass's throttle — an admin pressed the button and is waiting for
/// the answer. The catalog fetch and, for an OpenRouter endpoint, that
/// model's per-endpoint price list. The outcome lands in `price_lookups`
/// either way; an Err is a failure to look at all (the endpoint is gone or
/// not cloud), and a failed source is an Ok whose recorded outcome says so.
pub async fn lookup_now(pg: &PgPool, endpoint: &str, model: &str) -> Result<(), String> {
    let row: Option<(String, String)> = sqlx::query_as(
        "select id::text, provider from llm_endpoints where name = $1 and class = 'cloud'",
    )
    .bind(endpoint)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("endpoint read failed: {e}"))?;
    let Some((id, provider)) = row else {
        return Err(format!(
            "there is no cloud endpoint named {endpoint} to look prices up on"
        ));
    };
    if provider == "openrouter" {
        let _ = spend_fetch::ingest_openrouter_model(pg, &id, model).await;
    }
    // The outcome is recorded inside; a source failure is not this call's
    // error — the recorded row carries it.
    let _ = refresh_auto_prices(pg, Some((endpoint, model))).await;
    Ok(())
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
/// again. The scheduled job queuing behind a concurrent kick waits and
/// refreshes again — at most one
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

async fn refresh_once(
    pg: &PgPool,
    min_gap_ms: Option<i64>,
) -> Option<Result<RefreshCounts, String>> {
    let _flight = FLIGHT.lock().await;
    if min_gap_ms.is_some_and(|gap| !gate_open(&stamps(), now_ms(), gap)) {
        return None;
    }
    stamps().last_attempt = now_ms();
    let _ = spend_fetch::ingest_published_prices(pg).await;
    let result = refresh_auto_prices(pg, None).await;
    if result.is_ok() {
        stamps().last_refresh = now_ms();
    }
    Some(result)
}

/// Fire-and-forget: at most every 6h after a success,
/// and no retry storm when offline (10min backoff between failed attempts).
/// Call from hot read paths. Offline/unreachable → prices stay as they were.
pub fn maybe_refresh_auto_prices(pg: &PgPool) {
    let pg = pg.clone();
    tokio::spawn(async move {
        let _ = refresh_once(&pg, Some(PRICE_REFRESH_EVERY_MS as i64)).await;
    });
}

/// Usage just landed on a cloud model with NO price:
/// refresh sooner than the 6h cadence (new model, new alias), but never
/// storm.
pub fn nudge_auto_prices(pg: &PgPool) {
    let pg = pg.clone();
    tokio::spawn(async move {
        let _ = refresh_once(&pg, Some(NUDGE_INTERVAL_MS)).await;
    });
}

/// An IMMEDIATE ungated refresh —
/// admin edits to the endpoints table want fresh auto-prices now, not at the
/// next 6h tick. Fire-and-forget,
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

type KeyLoad = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = Vec<spend_fetch::KeyedEndpoint>> + Send>>
        + Send
        + Sync,
>;

/// Set from the api binary so the refresh job can open sealed provider keys
/// without this crate owning `AppState` on the job deps.
pub static LOAD_SPEND_KEYS: std::sync::OnceLock<KeyLoad> = std::sync::OnceLock::new();

pub async fn keys_for(state: &talaria_state::AppState) -> Vec<spend_fetch::KeyedEndpoint> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select id::text, provider, api_key_env from llm_endpoints where class = 'cloud'",
    )
    .fetch_all(&state.pg)
    .await
    .unwrap_or_default();
    let mut out = Vec::new();
    for (id, provider, api_key_env) in rows {
        if let Some(key) = talaria_gateway::provider::open_stored_key(
            state,
            &id,
            &provider,
            api_key_env.as_deref(),
        )
        .await
        {
            out.push(spend_fetch::KeyedEndpoint { id, provider, key });
        }
    }
    out
}

pub fn price_refresh_job_spec(deps: Arc<PriceRefreshDeps>) -> JobSpec {
    JobSpec {
        name: JobName::PriceRefresh,
        every_ms: PRICE_REFRESH_EVERY_MS,
        first_run_delay_ms: Some(PRICE_REFRESH_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(PRICE_REFRESH_MAX_RUN_MS),
        per_instance: false,
        run: Arc::new(move || {
            let pg = deps.pg.clone();
            Box::pin(async move {
                let mut keyed = Vec::new();
                let mut key_note = "";
                match LOAD_SPEND_KEYS.get() {
                    Some(load) => keyed = load().await,
                    None => {
                        // Warn once for the whole process: a missing loader
                        // is a wiring bug, not a per-tick event.
                        static WARNED_KEY_LOADER: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(false);
                        if !WARNED_KEY_LOADER.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            tracing::warn!(
                                "price-refresh: LOAD_SPEND_KEYS not wired; provider spend ingest skipped"
                            );
                        }
                        key_note = "; no key loader wired";
                    }
                }
                let spent = spend_fetch::ingest_provider_spend(&pg, &keyed).await;
                let spend_note = match &spent {
                    Ok(n) => format!("{n} provider spend row(s)"),
                    Err(e) => {
                        tracing::warn!("price-refresh: provider spend ingest failed: {e}");
                        format!("0 provider spend row(s); provider spend ingest failed: {e}")
                    }
                };
                let spend_note = format!("{spend_note}{key_note}");
                match refresh_once(&pg, None).await {
                    Some(Ok(c)) => {
                        let mut out = format!(
                            "{} published price(s), {spend_note}, {} catalog fill(s), \
                             {} catalog-priced model(s)",
                            c.priced, c.endpoints, c.catalog_priced
                        );
                        if c.upsert_failed > 0 {
                            tracing::warn!(
                                "price-refresh: {} catalog upsert(s) failed — first: {}",
                                c.upsert_failed,
                                c.first_upsert_error.as_deref().unwrap_or("unknown")
                            );
                            out.push_str(&format!(
                                ", {} catalog upsert(s) failed",
                                c.upsert_failed
                            ));
                        }
                        Ok(Some(out))
                    }
                    Some(Err(e)) => Err(e),
                    None => Ok(Some(spend_note)),
                }
            })
        }),
    }
}

pub fn register_price_refresh_job(deps: Arc<PriceRefreshDeps>) {
    talaria_scheduler::register_job(price_refresh_job_spec(deps));
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
        assert_eq!(
            id_candidates("gpt-5-2026-01-01"),
            ["gpt-5-2026-01-01", "gpt-5"]
        );
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
    fn a_catalog_hit_becomes_a_model_level_price_row() {
        let c = build_catalog(&[entry("openai/gpt-5", "1", "2")]);
        let hit = price_for(&c, "openai", "gpt-5").unwrap();
        let row = catalog_price_row("gpt-5", hit);
        // variant '' is the model-level rate — the one the spend view
        // matches when no serving variant is known.
        assert_eq!(row.model, "gpt-5");
        assert_eq!(row.variant, "");
        assert_eq!(row.source, "openrouter.catalog");
        assert!(near(row.price_in_per_mtok.unwrap(), 1e6));
        assert!(near(row.price_out_per_mtok.unwrap(), 2e6));
        // Stored rounded to four decimals like every other rate.
        let row = catalog_price_row(
            "m",
            CatalogPrice {
                in_per_mtok: 1.23456,
                out_per_mtok: 7.00006,
                cache_write_per_mtok: Some(1.543_21),
                cache_read_per_mtok: None,
            },
        );
        assert!(near(row.price_in_per_mtok.unwrap(), 1.2346));
        assert!(near(row.price_out_per_mtok.unwrap(), 7.0001));
        assert!(near(row.price_cache_write_per_mtok.unwrap(), 1.5432));
        assert_eq!(row.price_cache_read_per_mtok, None);
    }

    #[test]
    fn a_catalog_entry_carries_its_published_cache_rates() {
        // The live catalog's z-ai/glm-4.6 of 2026-11-03: cache read published
        // (0.2x input, not the 0.1x multiplier), no cache write.
        let c = build_catalog(&[json!({
            "id": "z-ai/glm-4.6",
            "pricing": { "prompt": "0.0000005", "completion": "0.000002", "input_cache_read": "0.0000001" }
        })]);
        let hit = price_for(&c, "custom", "glm-4.6").unwrap();
        assert!(near(hit.in_per_mtok, 0.5));
        assert!(near(hit.cache_read_per_mtok.unwrap(), 0.1));
        assert_eq!(hit.cache_write_per_mtok, None);
        assert_eq!(c.size, 1);
    }

    #[test]
    fn snapshot_dates_strip_in_every_shape_providers_stamp() {
        // Anthropic's -YYYYMMDD, OpenAI's -YYYY-MM-DD, xAI's / old OpenAI's -MMDD.
        assert_eq!(
            strip_date_suffix("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(strip_date_suffix("gpt-4.1-2025-04-14"), "gpt-4.1");
        assert_eq!(strip_date_suffix("gpt-5-mini-2025-08-07"), "gpt-5-mini");
        assert_eq!(strip_date_suffix("grok-4-0709"), "grok-4");
        assert_eq!(strip_date_suffix("gpt-4-0613"), "gpt-4");
        // A 4-digit tail that is not a month and day stays: Qwen's YYMM.
        assert_eq!(
            strip_date_suffix("qwen3-235b-a22b-2507"),
            "qwen3-235b-a22b-2507"
        );
        assert_eq!(
            strip_date_suffix("mistral-large-2411"),
            "mistral-large-2411"
        );
        assert_eq!(strip_date_suffix("model-1399"), "model-1399");
        // Not a month and day, or no dash right before the four digits.
        assert_eq!(strip_date_suffix("v-123-45-6789"), "v-123-45-6789");
        assert_eq!(strip_date_suffix("build-10709"), "build-10709");
        // Too short, or no dash: untouched.
        assert_eq!(strip_date_suffix("0709"), "0709");
        assert_eq!(strip_date_suffix("gpt-5"), "gpt-5");
    }

    #[test]
    fn real_provider_ids_reach_the_catalog_entry_they_are_sold_as() {
        // Ids as each provider's own /models lists them, against catalog
        // entries as OpenRouter's lists them (2026-11-03). Each of these was
        // unpriced before the date shapes and the last-segment step.
        let c = build_catalog(&[
            entry("openai/gpt-4.1", "0.000002", "0.000008"),
            entry("openai/gpt-5-mini", "0.00000025", "0.000002"),
            entry("x-ai/grok-4", "0.000003", "0.000015"),
            entry("deepseek/deepseek-v3", "0.0000003", "0.0000012"),
        ]);
        let in_rate = |provider: &str, id: &str| price_for(&c, provider, id).map(|p| p.in_per_mtok);
        assert!(near(in_rate("openai", "gpt-4.1-2025-04-14").unwrap(), 2.0));
        assert!(near(
            in_rate("openai", "gpt-5-mini-2025-08-07").unwrap(),
            0.25
        ));
        assert!(near(in_rate("x-ai", "grok-4-0709").unwrap(), 3.0));
        assert!(near(
            in_rate("custom", "accounts/fireworks/models/deepseek-v3").unwrap(),
            0.3
        ));
        // Still never a guess: an id with no catalog shape stays unpriced.
        assert!(in_rate("custom", "llama-3.3-70b-versatile").is_none());
    }

    #[test]
    fn a_multi_segment_path_also_tries_its_last_segment() {
        assert_eq!(
            id_candidates("accounts/fireworks/models/deepseek-v3"),
            ["accounts/fireworks/models/deepseek-v3", "deepseek-v3"]
        );
        // One slash is a vendor path, matched whole and by suffix already.
        assert_eq!(id_candidates("openai/gpt-5"), ["openai/gpt-5"]);
    }

    #[test]
    fn a_variable_price_is_no_price_and_the_miss_says_so() {
        // The live catalog's routers, verbatim (2026-11-03): "-1" is
        // OpenRouter's marker for "variable", never -$1 per token.
        let c = build_catalog(&[
            json!({ "id": "openrouter/auto", "pricing": { "prompt": "-1", "completion": "-1" } }),
            json!({ "id": "openrouter/fusion", "pricing": { "prompt": "-1", "completion": "-1" } }),
            json!({ "id": "nvidia/switchyard", "pricing": { "prompt": "-1", "completion": "-1" } }),
            json!({ "id": "typesafe/jev-router", "pricing": { "prompt": "-1", "completion": "-1" } }),
            entry("openai/gpt-4o", "0.0000025", "0.00001"),
        ]);
        for (provider, model) in [
            ("openrouter", "openrouter/auto"),
            ("openrouter", "openrouter/fusion"),
            ("custom", "nvidia/switchyard"),
            ("openrouter", "typesafe/jev-router"),
        ] {
            assert!(
                price_for(&c, provider, model).is_none(),
                "{model} must stay unpriced"
            );
            assert!(
                variable_entry(&c, model).is_some(),
                "{model} is recorded as variable"
            );
        }
        // A bare id is never a router: a custom endpoint's `auto` neither
        // prices at the router's -1 nor claims its reason.
        assert!(price_for(&c, "custom", "auto").is_none());
        assert!(price_for(&c, "custom", "fusion").is_none());
        assert_eq!(variable_entry(&c, "auto"), None);
        assert!(variable_detail("openrouter/auto").contains("variable price for openrouter/auto"));
        // Only real prices count toward the catalog's size.
        assert_eq!(c.size, 1);
        assert!(price_for(&c, "openrouter", "openai/gpt-4o").is_some());
    }

    #[test]
    fn negative_or_unreadable_numbers_are_never_prices() {
        assert_eq!(tok_per_token(Some(&json!("-1"))), None);
        assert_eq!(tok_per_token(Some(&json!(-0.5))), None);
        assert_eq!(tok_per_token(Some(&json!("NaN"))), None);
        assert_eq!(tok_per_token(Some(&json!("inf"))), None);
        assert_eq!(tok_per_token(Some(&json!("0"))), Some(0.0));
        assert_eq!(tok_per_token(Some(&json!("0.0000025"))), Some(0.0000025));
        // A negative cache rate drops the rate, not the entry.
        let c = build_catalog(&[json!({
            "id": "openai/gpt-4o",
            "pricing": { "prompt": "0.0000025", "completion": "0.00001", "input_cache_read": "-1", "input_cache_write": "-1" }
        })]);
        let hit = price_for(&c, "openai", "gpt-4o").unwrap();
        assert_eq!(
            (hit.cache_read_per_mtok, hit.cache_write_per_mtok),
            (None, None)
        );
        assert!(hit.in_per_mtok >= 0.0 && hit.out_per_mtok >= 0.0);
    }

    #[test]
    fn a_failure_names_every_cause_under_it() {
        #[derive(Debug)]
        struct E(&'static str, Option<Box<E>>);
        impl std::fmt::Display for E {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for E {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1.as_deref().map(|e| e as _)
            }
        }
        let e = E(
            "error sending request for url (https://openrouter.ai/api/v1/models)",
            Some(Box::new(E(
                "client error (Connect)",
                Some(Box::new(E(
                    "tcp connect error: Connection refused (os error 111)",
                    None,
                ))),
            ))),
        );
        assert_eq!(
            error_chain(&e),
            "error sending request for url (https://openrouter.ai/api/v1/models): client error (Connect): tcp connect error: Connection refused (os error 111)"
        );
    }

    #[test]
    fn a_miss_records_the_catalog_size_and_every_id_it_tried() {
        assert_eq!(
            no_match_detail("gpt-9-2027-01-01", 454),
            "OpenRouter's public catalog (454 priced models) has no entry for gpt-9-2027-01-01 (also tried gpt-9)"
        );
        assert_eq!(
            no_match_detail("llama-3.3-70b-versatile", 454),
            "OpenRouter's public catalog (454 priced models) has no entry for llama-3.3-70b-versatile"
        );
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

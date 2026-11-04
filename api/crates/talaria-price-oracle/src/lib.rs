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
#[derive(Debug, Clone, PartialEq)]
struct CatalogPrice {
    /// The catalog's own id for the entry — what a match was "priced as".
    id: String,
    in_per_mtok: f64,
    out_per_mtok: f64,
    cache_write_per_mtok: Option<f64>,
    cache_read_per_mtok: Option<f64>,
}

impl CatalogPrice {
    /// Same input/output rates — the test for "unambiguous" and for a
    /// snapshot that disagrees with its alias.
    fn same_rates(&self, other: &CatalogPrice) -> bool {
        self.in_per_mtok == other.in_per_mtok && self.out_per_mtok == other.out_per_mtok
    }
}

/// How a candidate id was derived from the model's own id — the match's
/// confidence. Only `AsIs` is an exact match; the rest are fuzzy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    AsIs,
    Tag,
    Date,
    Latest,
    Segment,
}

impl Step {
    /// The `provider_prices.match_kind` / `price_lookups.match_kind` word.
    fn kind(self) -> &'static str {
        match self {
            Step::AsIs => "exact",
            Step::Tag | Step::Date | Step::Latest => "alias",
            Step::Segment => "segment",
        }
    }
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
    id_steps(model).into_iter().map(|(id, _)| id).collect()
}

/// The candidates with how each was derived. The full id's chain first
/// (as-is → ":tag" stripped → date stripped → "-latest" stripped), then —
/// for a multi-segment path (`accounts/fireworks/models/<id>`) — the SAME
/// chain on its last segment, so `…/deepseek-r1-0528` tries
/// `deepseek-r1-0528` before `deepseek-r1`.
pub(crate) fn id_steps(model: &str) -> Vec<(String, Step)> {
    let mut out: Vec<(String, Step)> = Vec::new();
    let chain = |start: &str, first: Step, out: &mut Vec<(String, Step)>| {
        let push = |m: String, step: Step, out: &mut Vec<(String, Step)>| {
            if !m.is_empty() && !out.iter().any(|(id, _)| id == &m) {
                out.push((m, step));
            }
        };
        let fuzzy = |step: Step| {
            if first == Step::Segment {
                Step::Segment
            } else {
                step
            }
        };
        let mut cur = start.to_string();
        push(cur.clone(), first, out);
        if cur.contains(':') {
            cur = cur.split(':').next().unwrap_or(&cur).to_string();
            push(cur.clone(), fuzzy(Step::Tag), out);
        }
        let dateless = strip_date_suffix(&cur);
        if dateless != cur {
            cur = dateless;
            push(cur.clone(), fuzzy(Step::Date), out);
        }
        if let Some(unlatest) = cur.strip_suffix("-latest") {
            push(unlatest.to_string(), fuzzy(Step::Latest), out);
        }
    };
    let full = model.strip_prefix('~').unwrap_or(model);
    chain(full, Step::AsIs, &mut out);
    if full.matches('/').count() >= 2
        && let Some(last) = full.rsplit('/').next()
    {
        chain(last, Step::Segment, &mut out);
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
    /// Dated snapshots' prices by their undated base id (normalized), for
    /// the alias check: a snapshot the catalog lacks may borrow its alias's
    /// price only when every snapshot it does list agrees with that alias.
    snapshots: HashMap<String, Vec<CatalogPrice>>,
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
    let mut snapshots: HashMap<String, Vec<CatalogPrice>> = HashMap::new();
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
            id: id.to_string(),
            in_per_mtok: in_tok * 1e6,
            out_per_mtok: out_tok * 1e6,
            cache_write_per_mtok: tok_per_token(pricing.and_then(|p| p.get("input_cache_write")))
                .map(|p| p * 1e6),
            cache_read_per_mtok: tok_per_token(pricing.and_then(|p| p.get("input_cache_read")))
                .map(|p| p * 1e6),
        };
        let norm = normalize(id);
        let base = strip_date_suffix(&norm);
        if base != norm {
            snapshots.entry(base).or_default().push(price.clone());
        }
        by_id.insert(norm, price.clone());
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
        snapshots,
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
        return Some(hit.price.clone());
    }
    let first = &candidates[0].price;
    let unambiguous = candidates.iter().all(|c| c.price.same_rates(first));
    unambiguous.then(|| first.clone())
}

/// Pick a price for a model on a provider. Vendor-prefixed ids
/// match the full catalog id; bare ids match by suffix (exact vendor wins, else
/// only an unambiguous candidate). Alias shapes (":free" variants, "-latest",
/// trailing dates, "~" prefixes) fall back through id_candidates. None = no
/// match — stays unpriced, never guessed.
#[cfg(test)]
fn price_for(catalog: &Catalog, provider: &str, model: &str) -> Option<CatalogPrice> {
    id_steps(model)
        .into_iter()
        .find_map(|(id, _)| lookup(catalog, provider, &id))
}

/// One candidate id against the catalog: a vendor-prefixed id by its full
/// id, then by suffix under another vendor path; a bare id by suffix.
fn lookup(catalog: &Catalog, provider: &str, id: &str) -> Option<CatalogPrice> {
    if id.contains('/') {
        if let Some(hit) = catalog.by_id.get(&normalize(id)) {
            return Some(hit.clone());
        }
        let (vendor, rest) = id.split_once('/')?;
        return suffix_match(catalog, vendor, rest);
    }
    suffix_match(catalog, provider, id)
}

/// What the price pass does with one model.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    /// Store this price; `step` says how sure the match is.
    Price { hit: CatalogPrice, step: Step },
    /// A model on another host matched an OpenRouter entry only by its last
    /// path segment: OpenRouter's rate is not that host's, so it is offered
    /// to the admin, never applied.
    Suggest { hit: CatalogPrice },
    /// The catalog lists the model with a variable price.
    Variable { catalog_id: String },
    /// Nothing usable; the sentence says what was tried and why.
    NoMatch { detail: String },
}

/// THE MATCH POLICY over the raw matcher:
/// - on an OpenRouter endpoint a `:tag` id (`:online`, `:nitro`) is priced
///   only by its own catalog entry — OpenRouter lists every id it sells, and
///   the base model's rate leaves out what the tag adds (a web-search fee,
///   a faster and dearer provider);
/// - a dated snapshot the catalog lacks borrows its undated alias's price
///   only when every snapshot of it the catalog does list agrees with that
///   alias (`gpt-4o-2024-05-13` costs twice `gpt-4o`);
/// - a last-segment match prices only on OpenRouter; anywhere else it is a
///   suggestion.
pub(crate) fn decide(catalog: &Catalog, provider: &str, model: &str) -> Decision {
    let openrouter = provider == "openrouter";
    let mut notes: Vec<String> = Vec::new();
    for (id, step) in id_steps(model) {
        if openrouter && step == Step::Tag {
            notes.push(format!(
                "on OpenRouter a tagged id is priced only by its own entry, and the catalog has none for {model} (the base model's rate would leave out what the tag adds)"
            ));
            continue;
        }
        let Some(hit) = lookup(catalog, provider, &id) else {
            continue;
        };
        if matches!(step, Step::Date)
            && let Some(snaps) = catalog.snapshots.get(&normalize(&hit.id))
            && snaps.iter().any(|snap| !snap.same_rates(&hit))
        {
            notes.push(format!(
                "the catalog prices {}'s dated snapshots differently from {} itself, so a snapshot it does not list cannot borrow that price",
                hit.id, hit.id
            ));
            continue;
        }
        if step == Step::Segment && !openrouter {
            return Decision::Suggest { hit };
        }
        return Decision::Price { hit, step };
    }
    if let Some(catalog_id) = variable_entry(catalog, model) {
        return Decision::Variable { catalog_id };
    }
    let mut detail = no_match_detail(model, catalog.size);
    if !notes.is_empty() {
        detail.push_str("; ");
        detail.push_str(&notes.join("; "));
    }
    Decision::NoMatch { detail }
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
fn catalog_price_row(model: &str, hit: &CatalogPrice, step: Step) -> spend::PublishedPrice {
    spend::PublishedPrice {
        model: model.to_string(),
        variant: String::new(),
        price_in_per_mtok: Some(round4(hit.in_per_mtok)),
        price_out_per_mtok: Some(round4(hit.out_per_mtok)),
        price_cache_write_per_mtok: hit.cache_write_per_mtok.map(round4),
        price_cache_read_per_mtok: hit.cache_read_per_mtok.map(round4),
        source: "openrouter.catalog",
        matched_id: Some(hit.id.clone()),
        match_kind: Some(step.kind()),
    }
}

/// "priced as openai/gpt-4o (alias)" — what a catalog hit records.
fn priced_as_detail(hit: &CatalogPrice, step: Step) -> String {
    match step {
        Step::AsIs => format!("priced as {}", hit.id),
        _ => format!("priced as {} ({} match)", hit.id, step.kind()),
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
           source = excluded.source, checked_at = now(), \
           matched_id = null, suggest_in_per_mtok = null, suggest_out_per_mtok = null, \
           suggest_cache_write_per_mtok = null, suggest_cache_read_per_mtok = null",
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

/// A last-segment match on another host: OpenRouter's entry and rates,
/// recorded as a suggestion the admin may accept as a manual price.
async fn record_suggestion(
    pg: &PgPool,
    endpoint_id: &str,
    model: &str,
    hit: &CatalogPrice,
) -> Result<(), sqlx::Error> {
    let detail = format!(
        "OpenRouter lists {} under the same name. That is OpenRouter's rate, not necessarily this provider's, so it is offered here rather than applied.",
        hit.id
    );
    sqlx::query(
        "insert into price_lookups (endpoint_id, model, outcome, detail, source, checked_at, \
           matched_id, suggest_in_per_mtok, suggest_out_per_mtok, suggest_cache_write_per_mtok, \
           suggest_cache_read_per_mtok) \
         values ($1::uuid, $2, 'suggested', $3, $4, now(), $5, $6, $7, $8, $9) \
         on conflict (endpoint_id, model) do update set \
           outcome = excluded.outcome, detail = excluded.detail, source = excluded.source, \
           checked_at = now(), matched_id = excluded.matched_id, \
           suggest_in_per_mtok = excluded.suggest_in_per_mtok, \
           suggest_out_per_mtok = excluded.suggest_out_per_mtok, \
           suggest_cache_write_per_mtok = excluded.suggest_cache_write_per_mtok, \
           suggest_cache_read_per_mtok = excluded.suggest_cache_read_per_mtok",
    )
    .bind(endpoint_id)
    .bind(model)
    .bind(&detail)
    .bind(CATALOG_SOURCE)
    .bind(&hit.id)
    .bind(round4(hit.in_per_mtok))
    .bind(round4(hit.out_per_mtok))
    .bind(hit.cache_write_per_mtok.map(round4))
    .bind(hit.cache_read_per_mtok.map(round4))
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
            let decision = decide(&catalog, &provider, &m);
            if !matches!(decision, Decision::Price { .. }) {
                // No catalog price applies any more: drop an earlier pass's
                // row for this model — a "-1" stored before negatives were
                // refused, or a fuzzy match the policy now withholds — so it
                // stops pricing.
                let _ = sqlx::query(
                    "delete from provider_prices where endpoint_id = $1::uuid and model = $2 \
                     and variant = '' and source = $3",
                )
                .bind(&id)
                .bind(&m)
                .bind(CATALOG_SOURCE)
                .execute(pg)
                .await;
            }
            match decision {
                Decision::Price { hit, step } => {
                    let value = serde_json::to_value(AutoPrice {
                        in_per_mtok: round4(hit.in_per_mtok),
                        out_per_mtok: round4(hit.out_per_mtok),
                    })
                    .expect("an f64 pair serializes");
                    auto.insert(m.clone(), value);
                    counts.priced += 1;
                    // The same hit is also a published model-level price for
                    // this endpoint, carrying which catalog entry it is and
                    // how sure the match was.
                    let price = catalog_price_row(&m, &hit, step);
                    match spend_fetch::upsert_price(pg, &id, &provider, &price).await {
                        Ok(()) => {
                            counts.catalog_priced += 1;
                            let detail = priced_as_detail(&hit, step);
                            let _ = record_lookup(pg, &id, &m, "priced", Some(&detail)).await;
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
                }
                Decision::Suggest { hit } => {
                    counts.no_match += 1;
                    let _ = record_suggestion(pg, &id, &m, &hit).await;
                }
                Decision::Variable { catalog_id } => {
                    counts.no_match += 1;
                    let detail = variable_detail(&catalog_id);
                    let _ = record_lookup(pg, &id, &m, "variable", Some(&detail)).await;
                }
                Decision::NoMatch { detail } => {
                    counts.no_match += 1;
                    let _ = record_lookup(pg, &id, &m, "no_match", Some(&detail)).await;
                }
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
        let row = catalog_price_row("gpt-5", &hit, Step::AsIs);
        assert_eq!(row.matched_id.as_deref(), Some("openai/gpt-5"));
        assert_eq!(row.match_kind, Some("exact"));
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
            &CatalogPrice {
                id: "v/m".into(),
                in_per_mtok: 1.23456,
                out_per_mtok: 7.00006,
                cache_write_per_mtok: Some(1.543_21),
                cache_read_per_mtok: None,
            },
            Step::Date,
        );
        assert_eq!(row.match_kind, Some("alias"));
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
    fn a_dated_path_tries_its_own_dated_segment_before_the_undated_one() {
        // S3: the last segment's whole chain, dated id first.
        assert_eq!(
            id_candidates("accounts/fireworks/models/deepseek-r1-0528"),
            [
                "accounts/fireworks/models/deepseek-r1-0528",
                "accounts/fireworks/models/deepseek-r1",
                "deepseek-r1-0528",
                "deepseek-r1",
            ]
        );
        let c = build_catalog(&[entry(
            "deepseek/deepseek-r1-0528",
            "0.0000005",
            "0.00000215",
        )]);
        assert_eq!(
            price_for(&c, "custom", "accounts/fireworks/models/deepseek-r1-0528")
                .unwrap()
                .id,
            "deepseek/deepseek-r1-0528"
        );
        // Each candidate carries how it was derived.
        let steps: Vec<Step> = id_steps("accounts/fireworks/models/deepseek-r1-0528")
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        assert_eq!(
            steps,
            [Step::AsIs, Step::Date, Step::Segment, Step::Segment]
        );
        assert_eq!(
            id_steps("openai/gpt-4o:online")
                .into_iter()
                .map(|(_, s)| s)
                .collect::<Vec<_>>(),
            [Step::AsIs, Step::Tag]
        );
    }

    #[test]
    fn the_policy_marks_how_sure_a_match_is() {
        let c = build_catalog(&[
            entry("anthropic/claude-sonnet-4.5", "0.000003", "0.000015"),
            entry("openai/gpt-5", "0.00000125", "0.00001"),
        ]);
        // The model's own id (a bare id on its own vendor counts): exact.
        assert!(matches!(
            decide(&c, "anthropic", "claude-sonnet-4.5"),
            Decision::Price {
                step: Step::AsIs,
                ..
            }
        ));
        assert!(matches!(
            decide(&c, "openrouter", "anthropic/claude-sonnet-4.5"),
            Decision::Price {
                step: Step::AsIs,
                ..
            }
        ));
        // Reached through a stripped date: priced, and marked an alias.
        match decide(&c, "openai", "gpt-5-2026-01-01") {
            Decision::Price { hit, step } => {
                assert_eq!((hit.id.as_str(), step.kind()), ("openai/gpt-5", "alias"));
                assert_eq!(
                    priced_as_detail(&hit, step),
                    "priced as openai/gpt-5 (alias match)"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_snapshot_never_borrows_an_alias_its_listed_snapshots_disagree_with() {
        // The live catalog of 2026-11-03: the listed gpt-4o snapshot costs
        // twice the alias, so an unlisted snapshot cannot take the alias.
        let c = build_catalog(&[
            entry("openai/gpt-4o", "0.0000025", "0.00001"),
            entry("openai/gpt-4o-2024-05-13", "0.000005", "0.000015"),
            entry("openai/gpt-5", "0.00000125", "0.00001"),
            entry("openai/gpt-5-2025-08-07", "0.00000125", "0.00001"),
        ]);
        // Listed: exact, at its own price.
        match decide(&c, "openai", "gpt-4o-2024-05-13") {
            Decision::Price { hit, step } => {
                assert_eq!((hit.in_per_mtok, step), (5.0, Step::AsIs));
            }
            other => panic!("{other:?}"),
        }
        // Unlisted, and the family disagrees: unpriced, with the reason.
        match decide(&c, "openai", "gpt-4o-2024-11-20") {
            Decision::NoMatch { detail } => {
                assert!(detail.contains("dated snapshots differently"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
        // Unlisted, and every listed snapshot agrees with the alias: priced, fuzzy.
        assert!(matches!(
            decide(&c, "openai", "gpt-5-2026-02-02"),
            Decision::Price {
                step: Step::Date,
                ..
            }
        ));
    }

    #[test]
    fn a_tagged_openrouter_id_takes_only_its_own_entry() {
        // S5: the live catalog has no openai/gpt-4o:online.
        let c = build_catalog(&[
            entry("openai/gpt-4o", "0.0000025", "0.00001"),
            entry("recursive/flint-v1.0:free", "0", "0"),
        ]);
        match decide(&c, "openrouter", "openai/gpt-4o:online") {
            Decision::NoMatch { detail } => assert!(
                detail.contains("tagged id is priced only by its own entry"),
                "{detail}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            decide(&c, "openrouter", "openai/gpt-4o:nitro"),
            Decision::NoMatch { .. }
        ));
        // A tag the catalog lists is its own entry, exact.
        assert!(matches!(
            decide(&c, "openrouter", "recursive/flint-v1.0:free"),
            Decision::Price {
                step: Step::AsIs,
                ..
            }
        ));
        // Elsewhere a ":tag" is the endpoint's own spelling; the alias applies, marked fuzzy.
        assert!(matches!(
            decide(&c, "custom", "openai/gpt-4o:latest"),
            Decision::Price {
                step: Step::Tag,
                ..
            }
        ));
    }

    #[test]
    fn a_last_segment_match_on_another_host_is_a_suggestion() {
        let c = build_catalog(&[entry("openai/gpt-oss-120b", "0.00000002", "0.000000076")]);
        match decide(&c, "custom", "accounts/fireworks/models/gpt-oss-120b") {
            Decision::Suggest { hit } => assert_eq!(hit.id, "openai/gpt-oss-120b"),
            other => panic!("{other:?}"),
        }
        // On OpenRouter itself a segment match is OpenRouter's own rate.
        assert!(matches!(
            decide(&c, "openrouter", "x/models/gpt-oss-120b"),
            Decision::Price {
                step: Step::Segment,
                ..
            }
        ));
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

//! The unpriced list and an admin's pricing decisions.
//!
//! A cloud usage row is unpriced when the priced view (talaria-gateway
//! usage.rs) finds no provider charge, no admin decision and no published
//! price for its (endpoint, model). This module answers the admin's three
//! questions about those rows — which models, how much, and WHY — and stores
//! the two decisions an admin can make about one: a manual rate, or
//! "included in a plan / free" ($0, labelled). The third action, Retry, is
//! the oracle's own `lookup_now`.
//!
//! Nothing here stamps a cost onto a usage row. Pricing is read-time: a
//! decision saved here, or a price the oracle finds later, re-prices every
//! past row of that (endpoint, model) on the next read, which is what clears
//! the alert for history as well as new usage.

use serde::Serialize;
use sqlx::PgPool;
use talaria_gateway::usage::{AdminPricing, priced_view};

/// The window the list, the alert and the cost page all read.
pub const WINDOW_DAYS: i32 = 30;

/// The largest rate an admin may type, in $ per 1M tokens — a typo guard
/// (no model sells for $100k/MTok), not a policy.
pub const MAX_RATE_PER_MTOK: f64 = 100_000.0;

/// Why one model's usage has no price, as a machine code and the sentence a
/// person reads.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    pub code: &'static str,
    pub text: String,
    /// When the oracle last looked, when it has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
}

/// The oracle's last recorded conclusion for one endpoint's model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lookup<'a> {
    pub outcome: &'a str,
    pub detail: Option<&'a str>,
}

/// THE REASON DECISION: what to tell an admin about one unpriced (endpoint,
/// model), from what the ledger and the oracle know. Pure, so every branch
/// is pinned by a unit test. When a source spoke, its own words are the
/// sentence (quote, don't paraphrase); ours only frame them.
pub fn unpriced_reason(
    endpoint: &str,
    endpoint_exists: bool,
    model: &str,
    lookup: Option<Lookup<'_>>,
) -> Reason {
    let reason = |code: &'static str, text: String| Reason {
        code,
        text,
        checked_at: None,
    };
    if model.is_empty() {
        return reason(
            "no_model",
            "Booked without a model id, so no published price can be matched to it.".into(),
        );
    }
    if endpoint.is_empty() {
        return reason(
            "no_endpoint",
            "Booked without an endpoint: older releases did not record which endpoint served a model \
             that more than one endpoint carries, so no endpoint's published price applies."
                .into(),
        );
    }
    if !endpoint_exists {
        return reason(
            "endpoint_gone",
            format!(
                "The endpoint {endpoint} no longer exists, so the prices published for it went with it."
            ),
        );
    }
    match lookup {
        None => reason(
            "not_checked",
            "Not looked up yet. The price check runs every 6 hours and soon after new usage."
                .into(),
        ),
        Some(Lookup {
            outcome: "source_failed",
            detail,
        }) => reason(
            "source_failed",
            format!(
                "The price source didn't answer: {}",
                detail.unwrap_or("no reason was given")
            ),
        ),
        Some(Lookup {
            outcome: "suggested",
            detail,
        }) => reason(
            "suggested",
            format!(
                "Not applied automatically: {}",
                detail.unwrap_or("OpenRouter lists a model under the same name.")
            ),
        ),
        Some(Lookup {
            outcome: "variable",
            detail,
        }) => reason(
            "variable_price",
            format!(
                "No per-token price: {}",
                detail.unwrap_or("the source publishes a variable price for this model.")
            ),
        ),
        Some(Lookup {
            outcome: "no_match",
            detail,
        }) => reason(
            "no_match",
            format!(
                "No published price found. {}",
                detail.unwrap_or("The catalog has no entry for this id.")
            ),
        ),
        Some(_) => reason(
            "stale",
            "A price was found when it was last checked, and it does not cover these rows. \
             Retry the lookup."
                .into(),
        ),
    }
}

/// One unpriced model over the window.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnpricedModel {
    /// The ledger's endpoint text, '' when the rows carry none.
    pub endpoint: String,
    pub endpoint_exists: bool,
    pub provider: Option<String>,
    pub model: String,
    pub generations: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    pub tokens: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub reason: Reason,
    /// OpenRouter's entry for a model another host serves under the same
    /// last path segment — offered as a manual price, never applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<Suggestion>,
}

/// A suggested price: the catalog entry and its rates per 1M tokens.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub matched_id: String,
    #[serde(rename = "in")]
    pub price_in_per_mtok: f64,
    #[serde(rename = "out")]
    pub price_out_per_mtok: f64,
    pub cache_write: Option<f64>,
    pub cache_read: Option<f64>,
}

/// A model priced by a FUZZY catalog match over the window: the entry it
/// was priced as and how the match was made — shown so an admin who knows
/// the real rate can set it (a manual rate beats a fuzzy match).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FuzzyPriced {
    pub endpoint: String,
    pub model: String,
    pub matched_id: String,
    pub match_kind: String,
    pub generations: i64,
    pub tokens: i64,
    pub cost: f64,
}

/// An admin's decision, with what it prices now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceSetting {
    pub endpoint: String,
    pub model: String,
    pub mode: &'static str,
    #[serde(rename = "in")]
    pub price_in_per_mtok: Option<f64>,
    #[serde(rename = "out")]
    pub price_out_per_mtok: Option<f64>,
    pub cache_write: Option<f64>,
    pub cache_read: Option<f64>,
    pub note: Option<String>,
    pub set_by: Option<String>,
    pub updated_at: Option<String>,
    /// Rows in the window this decision prices now (basis `manual` or
    /// `included`). A manual rate that a published price has since covered
    /// prices none — `shadowed` says so.
    pub rows_priced: i64,
    pub tokens_priced: i64,
    pub shadowed: bool,
}

/// What a decision or a retry did to the window's history for one (endpoint,
/// model): every row, how many are priced now, and their dollars.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Repriced {
    pub rows: i64,
    pub priced_rows: i64,
    pub unpriced_rows: i64,
    pub tokens: i64,
    pub cost: f64,
    /// The basis the rows now carry, when they all agree.
    pub basis: Option<String>,
}

fn mode_str(m: AdminPricing) -> &'static str {
    match m {
        AdminPricing::Manual => "manual",
        AdminPricing::Included => "included",
    }
}

fn mode_of(s: &str) -> Option<AdminPricing> {
    match s {
        "manual" => Some(AdminPricing::Manual),
        "included" => Some(AdminPricing::Included),
        _ => None,
    }
}

/// An admin's decision as the route parsed it.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceInput {
    pub endpoint: String,
    pub model: String,
    pub mode: AdminPricing,
    pub price_in_per_mtok: Option<f64>,
    pub price_out_per_mtok: Option<f64>,
    pub price_cache_write_per_mtok: Option<f64>,
    pub price_cache_read_per_mtok: Option<f64>,
    pub note: Option<String>,
}

/// Validate a decision's MEANING (the route has already checked types and
/// lengths). A manual rate needs input and output; cache rates are optional
/// (the view falls back to the multipliers). "Included" is $0 by definition,
/// so a rate beside it is a contradiction, refused rather than ignored. The
/// Err is the 400's sentence.
pub fn validate_price(p: &PriceInput) -> Result<(), String> {
    if p.model.trim().is_empty() {
        return Err("model is required".into());
    }
    let rates = [
        ("priceInPerMtok", p.price_in_per_mtok),
        ("priceOutPerMtok", p.price_out_per_mtok),
        ("priceCacheWritePerMtok", p.price_cache_write_per_mtok),
        ("priceCacheReadPerMtok", p.price_cache_read_per_mtok),
    ];
    for (key, v) in rates {
        if let Some(v) = v
            && !(v.is_finite() && (0.0..=MAX_RATE_PER_MTOK).contains(&v))
        {
            return Err(format!(
                "{key} must be a dollar amount per 1M tokens between 0 and {MAX_RATE_PER_MTOK}"
            ));
        }
    }
    match p.mode {
        AdminPricing::Manual => {
            if p.price_in_per_mtok.is_none() || p.price_out_per_mtok.is_none() {
                return Err("a manual price needs priceInPerMtok and priceOutPerMtok".into());
            }
            // A $0 rate would read as a silent zero; "included" is the
            // labelled way to say it costs nothing.
            if p.price_in_per_mtok == Some(0.0) && p.price_out_per_mtok == Some(0.0) {
                return Err(
                    "a manual price of $0 in and $0 out is \"included in a plan\" — mark it included instead"
                        .into(),
                );
            }
        }
        AdminPricing::Included => {
            if rates.iter().any(|(_, v)| v.is_some()) {
                return Err(
                    "a model included in a plan is priced at $0 — send no rates with it".into(),
                );
            }
        }
    }
    Ok(())
}

/// Every unpriced cloud (endpoint, model) in the window, most tokens first,
/// each with its reason.
pub async fn list_unpriced(pg: &PgPool) -> Result<Vec<UnpricedModel>, sqlx::Error> {
    list_unpriced_where(pg, None).await
}

/// The unpriced list, or just one (endpoint, model)'s row of it — the filter
/// is in the scan, so the one-row read after a retry does not walk the
/// whole window.
async fn list_unpriced_where(
    pg: &PgPool,
    only: Option<(&str, &str)>,
) -> Result<Vec<UnpricedModel>, sqlx::Error> {
    type Row = (
        String,
        String,
        i64,
        i64,
        i64,
        i64,
        i64,
        Option<String>,
        Option<String>,
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
    );
    let view = priced_view();
    // AssertSqlSafe: the only interpolation is the priced view's constant
    // text; the window and the filter are binds.
    let sql = format!(
        "with priced as ({view}), \
         gaps as ( \
           select coalesce(endpoint, '') as endpoint, coalesce(llm_model, '') as model, \
                  count(*)::bigint as generations, \
                  coalesce(sum(prompt_tokens), 0)::bigint as prompt, \
                  coalesce(sum(completion_tokens), 0)::bigint as completion, \
                  coalesce(sum(cache_write_tokens), 0)::bigint as cache_write, \
                  coalesce(sum(cache_read_tokens), 0)::bigint as cache_read, \
                  min(created_at) as first_at, max(created_at) as last_at \
           from priced \
           where created_at > now() - make_interval(days => $1) \
             and endpoint_class = 'cloud' and cost is null \
             and ($2::text is null or (coalesce(endpoint, '') = $2 and llm_model = $3)) \
           group by 1, 2 \
         ) \
         select g.endpoint, g.model, g.generations, g.prompt, g.completion, g.cache_write, \
                g.cache_read, \
                to_char(g.first_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), \
                to_char(g.last_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), \
                (e.id is not null), e.provider, l.outcome, l.detail, \
                to_char(l.checked_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), \
                case when l.outcome = 'suggested' and l.matched_id is not null \
                          and l.suggest_in_per_mtok >= 0 and l.suggest_out_per_mtok >= 0 \
                     then jsonb_build_object('matchedId', l.matched_id, \
                            'in', l.suggest_in_per_mtok::float8, 'out', l.suggest_out_per_mtok::float8, \
                            'cacheWrite', l.suggest_cache_write_per_mtok::float8, \
                            'cacheRead', l.suggest_cache_read_per_mtok::float8) end \
         from gaps g \
         left join llm_endpoints e on e.name = g.endpoint \
         left join price_lookups l on l.endpoint_id = e.id and l.model = g.model \
         order by (g.prompt + g.completion + g.cache_write + g.cache_read) desc, g.endpoint, g.model"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(WINDOW_DAYS)
        .bind(only.map(|o| o.0))
        .bind(only.map(|o| o.1))
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                endpoint,
                model,
                generations,
                prompt,
                completion,
                cache_write,
                cache_read,
                first_seen,
                last_seen,
                endpoint_exists,
                provider,
                outcome,
                detail,
                checked_at,
                suggestion,
            )| {
                let suggestion: Option<Suggestion> =
                    suggestion.and_then(|v| serde_json::from_value(v).ok());
                let lookup = outcome.as_deref().map(|outcome| Lookup {
                    outcome,
                    detail: detail.as_deref(),
                });
                let mut reason = unpriced_reason(&endpoint, endpoint_exists, &model, lookup);
                reason.checked_at = checked_at;
                UnpricedModel {
                    endpoint,
                    endpoint_exists,
                    provider,
                    model,
                    generations,
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    cache_write_tokens: cache_write,
                    cache_read_tokens: cache_read,
                    tokens: prompt + completion + cache_write + cache_read,
                    first_seen,
                    last_seen,
                    reason,
                    suggestion,
                }
            },
        )
        .collect())
}

/// Every admin decision, with what it prices in the window right now.
pub async fn list_settings(pg: &PgPool) -> Result<Vec<PriceSetting>, sqlx::Error> {
    type Row = (
        String,
        String,
        String,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        i64,
        i64,
    );
    let view = priced_view();
    // AssertSqlSafe: the priced view's constant text only; the window binds.
    let sql = format!(
        "with priced as ({view}) \
         select o.endpoint, o.model, o.mode, o.price_in_per_mtok::float8, \
                o.price_out_per_mtok::float8, o.price_cache_write_per_mtok::float8, \
                o.price_cache_read_per_mtok::float8, o.note, o.set_by, \
                to_char(o.updated_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), \
                count(p.id) filter (where p.cost_basis = o.mode)::bigint, \
                coalesce(sum(p.prompt_tokens + p.completion_tokens + p.cache_write_tokens \
                             + p.cache_read_tokens) filter (where p.cost_basis = o.mode), 0)::bigint, \
                count(p.id)::bigint \
         from model_price_overrides o \
         left join priced p on coalesce(p.endpoint, '') = o.endpoint and p.llm_model = o.model \
              and p.endpoint_class = 'cloud' \
              and p.created_at > now() - make_interval(days => $1) \
         group by o.endpoint, o.model, o.mode, o.price_in_per_mtok, o.price_out_per_mtok, \
                  o.price_cache_write_per_mtok, o.price_cache_read_per_mtok, o.note, o.set_by, \
                  o.updated_at \
         order by o.updated_at desc"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(WINDOW_DAYS)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(
                endpoint,
                model,
                mode,
                pin,
                pout,
                pcw,
                pcr,
                note,
                set_by,
                updated_at,
                priced,
                tokens,
                all,
            )| {
                Some(PriceSetting {
                    endpoint,
                    model,
                    mode: mode_str(mode_of(&mode)?),
                    price_in_per_mtok: pin,
                    price_out_per_mtok: pout,
                    cache_write: pcw,
                    cache_read: pcr,
                    note,
                    set_by,
                    updated_at,
                    rows_priced: priced,
                    tokens_priced: tokens,
                    // It has usage to price, and prices none of it: a
                    // provider charge or a published price got there first.
                    shadowed: all > 0 && priced == 0,
                })
            },
        )
        .collect())
}

/// Save a decision (insert or replace). The caller validated it.
pub async fn set_price(pg: &PgPool, p: &PriceInput, actor: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into model_price_overrides (endpoint, model, mode, price_in_per_mtok, \
           price_out_per_mtok, price_cache_write_per_mtok, price_cache_read_per_mtok, note, \
           set_by, updated_at) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
         on conflict (endpoint, model) do update set \
           mode = excluded.mode, price_in_per_mtok = excluded.price_in_per_mtok, \
           price_out_per_mtok = excluded.price_out_per_mtok, \
           price_cache_write_per_mtok = excluded.price_cache_write_per_mtok, \
           price_cache_read_per_mtok = excluded.price_cache_read_per_mtok, \
           note = excluded.note, set_by = excluded.set_by, updated_at = now()",
    )
    .bind(&p.endpoint)
    .bind(&p.model)
    .bind(mode_str(p.mode))
    .bind(p.price_in_per_mtok)
    .bind(p.price_out_per_mtok)
    .bind(p.price_cache_write_per_mtok)
    .bind(p.price_cache_read_per_mtok)
    .bind(p.note.as_deref().filter(|n| !n.trim().is_empty()))
    .bind(actor)
    .execute(pg)
    .await
    .map(|_| ())
}

/// Remove a decision. False when there was none to remove.
pub async fn clear_price(pg: &PgPool, endpoint: &str, model: &str) -> Result<bool, sqlx::Error> {
    sqlx::query("delete from model_price_overrides where endpoint = $1 and model = $2")
        .bind(endpoint)
        .bind(model)
        .execute(pg)
        .await
        .map(|r| r.rows_affected() > 0)
}

/// How the window's rows for one (endpoint, model) price right now — the
/// "it re-priced N past rows" every write answers with.
pub async fn repriced(pg: &PgPool, endpoint: &str, model: &str) -> Result<Repriced, sqlx::Error> {
    let view = priced_view();
    // AssertSqlSafe: the priced view's constant text only; the rest binds.
    let sql = format!(
        "with priced as ({view}) \
         select count(*)::bigint, count(cost)::bigint, \
                coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens \
                             + cache_read_tokens), 0)::bigint, \
                coalesce(sum(cost), 0)::float8, \
                case when count(distinct cost_basis) = 1 and count(cost) = count(*) \
                     then min(cost_basis) end \
         from priced \
         where coalesce(endpoint, '') = $1 and llm_model = $2 and endpoint_class = 'cloud' \
           and created_at > now() - make_interval(days => $3)"
    );
    let (rows, priced_rows, tokens, cost, basis): (i64, i64, i64, f64, Option<String>) =
        sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(endpoint)
            .bind(model)
            .bind(WINDOW_DAYS)
            .fetch_one(pg)
            .await?;
    Ok(Repriced {
        rows,
        priced_rows,
        unpriced_rows: rows - priced_rows,
        tokens,
        cost,
        basis,
    })
}

/// The reason one (endpoint, model) is still unpriced after a retry, or None
/// when it is priced now.
pub async fn reason_now(
    pg: &PgPool,
    endpoint: &str,
    model: &str,
) -> Result<Option<Reason>, sqlx::Error> {
    Ok(list_unpriced_where(pg, Some((endpoint, model)))
        .await?
        .into_iter()
        .next()
        .map(|m| m.reason))
}

/// Every model the window prices by a fuzzy catalog match, most tokens first.
pub async fn list_fuzzy(pg: &PgPool) -> Result<Vec<FuzzyPriced>, sqlx::Error> {
    let view = priced_view();
    // AssertSqlSafe: the priced view's constant text only; the window binds.
    let sql = format!(
        "with priced as ({view}) \
         select coalesce(endpoint, ''), llm_model, cost_matched_id, cost_match_kind, \
                count(*)::bigint, \
                coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens \
                             + cache_read_tokens), 0)::bigint, \
                coalesce(sum(cost), 0)::float8 \
         from priced \
         where created_at > now() - make_interval(days => $1) \
           and cost_basis = 'derived' and cost_match_kind in ('alias', 'segment') \
           and llm_model is not null and cost_matched_id is not null \
         group by 1, 2, 3, 4 \
         order by 6 desc"
    );
    let rows: Vec<(String, String, String, String, i64, i64, f64)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(WINDOW_DAYS)
            .fetch_all(pg)
            .await?;
    Ok(rows
        .into_iter()
        .map(
            |(endpoint, model, matched_id, match_kind, generations, tokens, cost)| FuzzyPriced {
                endpoint,
                model,
                matched_id,
                match_kind,
                generations,
                tokens,
                cost,
            },
        )
        .collect())
}

/// One decision as it stands, for the audit's `before`: None when there is
/// none.
pub async fn setting_json(
    pg: &PgPool,
    endpoint: &str,
    model: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row: Option<(
        String,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<String>,
    )> = sqlx::query_as(
        "select mode, price_in_per_mtok::float8, price_out_per_mtok::float8, \
                    price_cache_write_per_mtok::float8, price_cache_read_per_mtok::float8, note \
             from model_price_overrides where endpoint = $1 and model = $2",
    )
    .bind(endpoint)
    .bind(model)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(mode, pin, pout, pcw, pcr, note)| {
        serde_json::json!({
            "mode": mode,
            "priceInPerMtok": pin,
            "priceOutPerMtok": pout,
            "priceCacheWritePerMtok": pcw,
            "priceCacheReadPerMtok": pcr,
            "note": note,
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(mode: AdminPricing) -> PriceInput {
        PriceInput {
            endpoint: "openai".into(),
            model: "gpt-4.1-2025-04-14".into(),
            mode,
            price_in_per_mtok: None,
            price_out_per_mtok: None,
            price_cache_write_per_mtok: None,
            price_cache_read_per_mtok: None,
            note: None,
        }
    }

    #[test]
    fn every_reason_branch_says_what_is_missing() {
        let r = |endpoint, exists, model, lookup| unpriced_reason(endpoint, exists, model, lookup);
        assert_eq!(r("openai", true, "", None).code, "no_model");
        assert_eq!(r("", false, "gpt-5-mini", None).code, "no_endpoint");
        let gone = r("old-openrouter", false, "anthropic/claude-sonnet-4.5", None);
        assert_eq!(gone.code, "endpoint_gone");
        assert!(gone.text.contains("old-openrouter"));
        assert_eq!(r("openai", true, "m", None).code, "not_checked");
        // A failed source is quoted, not paraphrased.
        let failed = r(
            "openai",
            true,
            "m",
            Some(Lookup {
                outcome: "source_failed",
                detail: Some("openrouter catalog fetch timed out"),
            }),
        );
        assert_eq!(failed.code, "source_failed");
        assert_eq!(
            failed.text,
            "The price source didn't answer: openrouter catalog fetch timed out"
        );
        let miss = r(
            "openai",
            true,
            "m",
            Some(Lookup {
                outcome: "no_match",
                detail: Some("OpenRouter's public catalog (454 priced models) has no entry for m"),
            }),
        );
        assert_eq!(miss.code, "no_match");
        assert!(
            miss.text
                .starts_with("No published price found. OpenRouter's public catalog")
        );
        // A router the catalog prices as variable: unpriced, and said so.
        let variable = r(
            "openrouter",
            true,
            "openrouter/auto",
            Some(Lookup {
                outcome: "variable",
                detail: Some("OpenRouter's catalog publishes a variable price for openrouter/auto"),
            }),
        );
        assert_eq!(variable.code, "variable_price");
        let suggested = r(
            "fireworks",
            true,
            "accounts/fireworks/models/gpt-oss-120b",
            Some(Lookup {
                outcome: "suggested",
                detail: Some("OpenRouter lists openai/gpt-oss-120b under the same name."),
            }),
        );
        assert_eq!(suggested.code, "suggested");
        assert!(
            suggested
                .text
                .starts_with("Not applied automatically: OpenRouter lists")
        );
        assert!(
            variable
                .text
                .starts_with("No per-token price: OpenRouter's catalog publishes a variable price")
        );
        // Priced when last checked, yet these rows are not: retry.
        assert_eq!(
            r(
                "openai",
                true,
                "m",
                Some(Lookup {
                    outcome: "priced",
                    detail: None
                })
            )
            .code,
            "stale"
        );
        // The endpoint question comes before the lookup's: a deleted
        // endpoint's old lookup row went with it anyway.
        assert_eq!(
            r(
                "gone",
                false,
                "m",
                Some(Lookup {
                    outcome: "no_match",
                    detail: None
                })
            )
            .code,
            "endpoint_gone"
        );
    }

    #[test]
    fn a_manual_price_needs_input_and_output_and_cache_rates_are_optional() {
        let mut p = input(AdminPricing::Manual);
        assert!(
            validate_price(&p)
                .unwrap_err()
                .contains("priceInPerMtok and priceOutPerMtok")
        );
        p.price_in_per_mtok = Some(2.0);
        assert!(validate_price(&p).is_err());
        p.price_out_per_mtok = Some(8.0);
        assert_eq!(validate_price(&p), Ok(()));
        p.price_cache_read_per_mtok = Some(0.5);
        p.price_cache_write_per_mtok = Some(0.0);
        assert_eq!(validate_price(&p), Ok(()));
        // $0 / $0 is "included", said by name — never a silent manual zero.
        p.price_in_per_mtok = Some(0.0);
        p.price_out_per_mtok = Some(0.0);
        assert!(validate_price(&p).unwrap_err().contains("mark it included"));
        p.price_out_per_mtok = Some(0.1);
        assert_eq!(validate_price(&p), Ok(()), "one free side is a real rate");
    }

    #[test]
    fn rates_are_finite_non_negative_and_bounded() {
        let mut p = input(AdminPricing::Manual);
        p.price_in_per_mtok = Some(-1.0);
        p.price_out_per_mtok = Some(1.0);
        assert!(
            validate_price(&p)
                .unwrap_err()
                .starts_with("priceInPerMtok")
        );
        p.price_in_per_mtok = Some(f64::NAN);
        assert!(validate_price(&p).is_err());
        p.price_in_per_mtok = Some(MAX_RATE_PER_MTOK + 1.0);
        assert!(validate_price(&p).is_err());
        p.price_in_per_mtok = Some(MAX_RATE_PER_MTOK);
        assert_eq!(validate_price(&p), Ok(()));
    }

    #[test]
    fn included_is_zero_by_definition_and_refuses_a_rate_beside_it() {
        let mut p = input(AdminPricing::Included);
        p.note = Some("Z.ai Coding Plan".into());
        assert_eq!(validate_price(&p), Ok(()));
        p.price_in_per_mtok = Some(1.0);
        assert!(validate_price(&p).unwrap_err().contains("$0"));
    }

    #[test]
    fn a_model_is_required_but_an_endpoint_may_be_the_empty_ledger_value() {
        let mut p = input(AdminPricing::Included);
        p.endpoint = String::new();
        assert_eq!(
            validate_price(&p),
            Ok(()),
            "'' prices rows booked without an endpoint"
        );
        p.model = "  ".into();
        assert_eq!(validate_price(&p).unwrap_err(), "model is required");
    }
}

/// LIVE: the real tables and the real priced view, rows booked through the
/// gateway's own ledger writer (`record_relay_usage`) with model ids real
/// providers return. `#[ignore]`d, so the plain test run skips them; ci.yml's
/// api job runs the ignored lib tests in its live-DB steps, against its
/// migrated scratch database (`cargo test --workspace --lib -- --ignored`).
///
///   DATABASE_URL=postgres://… cargo test -p talaria-price-oracle --lib -- \
///     --ignored unpriced::live
///
/// Every row a test makes is under its own endpoint name, and every claim is
/// about those rows — a sibling's rows in the same database cannot move it.
#[cfg(test)]
mod live {
    use super::*;
    use talaria_gateway::usage::{
        CostBasis, PriceFacts, RELAY_SOURCE, ReportedSpend, StatedFlow, TokenCounts,
        UNATTENDED_SOURCE, price_basis, record_relay_usage,
    };

    async fn pg() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        PgPool::connect(&url).await.expect("connect")
    }

    /// A unique endpoint name for one test's rows.
    fn name(tag: &str) -> String {
        format!("unpriced-live-{tag}-{}", talaria_agent_auth::now_ms())
    }

    async fn endpoint(pg: &PgPool, name: &str, provider: &str, models: &[&str]) -> String {
        let (id,): (String,) = sqlx::query_as(
            "insert into llm_endpoints (name, provider, class, models) \
             values ($1, $2, 'cloud', $3::jsonb) returning id::text",
        )
        .bind(name)
        .bind(provider)
        .bind(serde_json::json!(models).to_string())
        .fetch_one(pg)
        .await
        .expect("endpoint");
        id
    }

    async fn published(
        pg: &PgPool,
        id: &str,
        model: &str,
        variant: &str,
        inn: f64,
        out: f64,
        read: Option<f64>,
    ) {
        sqlx::query(
            "insert into provider_prices (endpoint_id, provider, model, variant, price_in_per_mtok, \
               price_out_per_mtok, price_cache_read_per_mtok, source) \
             values ($1::uuid, 'test', $2, $3, $4, $5, $6, 'openrouter.catalog')",
        )
        .bind(id)
        .bind(model)
        .bind(variant)
        .bind(inn)
        .bind(out)
        .bind(read)
        .execute(pg)
        .await
        .expect("published price");
    }

    /// One relay row, written by the relay's own writer.
    async fn book(
        pg: &PgPool,
        endpoint: &str,
        model: &str,
        counts: TokenCounts,
        spend: Option<ReportedSpend>,
    ) {
        record_relay_usage(
            pg,
            RELAY_SOURCE,
            &format!("{endpoint}:caller"),
            None,
            &StatedFlow::default(),
            endpoint,
            "cloud",
            model,
            &counts,
            false,
            spend.as_ref(),
        )
        .await
        .expect("the relay's insert");
    }

    fn tokens(prompt: i64, completion: i64, cache_read: i64) -> TokenCounts {
        TokenCounts {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cache_read_tokens: cache_read,
            ..TokenCounts::default()
        }
    }

    /// Each row of one endpoint as the view prices it: (model, cost, basis).
    async fn priced_rows(
        pg: &PgPool,
        endpoint: &str,
    ) -> Vec<(String, Option<f64>, Option<String>)> {
        let sql = format!(
            "with priced as ({}) select llm_model, cost::float8, cost_basis from priced \
             where endpoint = $1 order by created_at, id",
            priced_view()
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(endpoint)
            .fetch_all(pg)
            .await
            .expect("priced rows")
    }

    async fn cleanup(pg: &PgPool, endpoint: &str) {
        for sql in [
            "delete from usage_events where endpoint = $1",
            "delete from model_price_overrides where endpoint = $1",
            "delete from llm_endpoints where name = $1",
        ] {
            sqlx::query(sql).bind(endpoint).execute(pg).await.unwrap();
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_manual_rate_beats_a_fuzzy_match_but_not_an_exact_one() {
        let pg = pg().await;
        let ep = name("fuzzy");
        let id = endpoint(&pg, &ep, "openai", &[]).await;
        // gpt-4o-2024-11-20 priced through its alias, and gpt-4o by its own id.
        published(&pg, &id, "gpt-4o-2024-11-20", "", 2.5, 10.0, None).await;
        published(&pg, &id, "gpt-4o", "", 2.5, 10.0, None).await;
        sqlx::query(
            "update provider_prices set match_kind = case model when 'gpt-4o' then 'exact' else 'alias' end, \
                    matched_id = 'openai/gpt-4o' where endpoint_id = $1::uuid",
        )
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
        book(&pg, &ep, "gpt-4o-2024-11-20", tokens(1_000_000, 0, 0), None).await;
        book(&pg, &ep, "gpt-4o", tokens(1_000_000, 0, 0), None).await;
        // The fuzzy row is listed, priced as the alias.
        let fuzzy = list_fuzzy(&pg).await.unwrap();
        let f = fuzzy
            .iter()
            .find(|f| f.endpoint == ep)
            .expect("the fuzzy match is listed");
        assert_eq!(
            (
                f.model.as_str(),
                f.matched_id.as_str(),
                f.match_kind.as_str()
            ),
            ("gpt-4o-2024-11-20", "openai/gpt-4o", "alias")
        );
        assert!(
            fuzzy
                .iter()
                .all(|f| !(f.endpoint == ep && f.model == "gpt-4o")),
            "an exact match is not fuzzy"
        );
        // The admin knows the snapshot's real rate: it beats the guess...
        for model in ["gpt-4o-2024-11-20", "gpt-4o"] {
            set_price(
                &pg,
                &PriceInput {
                    endpoint: ep.clone(),
                    model: model.into(),
                    mode: AdminPricing::Manual,
                    price_in_per_mtok: Some(5.0),
                    price_out_per_mtok: Some(15.0),
                    price_cache_write_per_mtok: None,
                    price_cache_read_per_mtok: None,
                    note: None,
                },
                "live-test",
            )
            .await
            .unwrap();
        }
        let rows = priced_rows(&pg, &ep).await;
        let basis = |m: &str| rows.iter().find(|r| r.0 == m).and_then(|r| r.2.clone());
        assert_eq!(basis("gpt-4o-2024-11-20").as_deref(), Some("manual"));
        assert!(near(
            rows.iter()
                .find(|r| r.0 == "gpt-4o-2024-11-20")
                .unwrap()
                .1
                .unwrap(),
            5.0
        ));
        // ...but not the provider's published number for the model itself.
        assert_eq!(basis("gpt-4o").as_deref(), Some("derived"));
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_suggested_price_is_offered_not_applied() {
        let pg = pg().await;
        let ep = name("suggest");
        let id = endpoint(&pg, &ep, "custom", &[]).await;
        let model = "accounts/fireworks/models/gpt-oss-120b";
        book(&pg, &ep, model, tokens(100_000, 10_000, 0), None).await;
        let hit = crate::CatalogPrice {
            id: "openai/gpt-oss-120b".into(),
            in_per_mtok: 0.02,
            out_per_mtok: 0.076,
            cache_write_per_mtok: None,
            cache_read_per_mtok: Some(0.018),
        };
        crate::record_suggestion(&pg, &id, model, &hit)
            .await
            .unwrap();
        let row = list_unpriced(&pg)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.endpoint == ep)
            .expect("still unpriced: a suggestion is never applied");
        assert_eq!(row.reason.code, "suggested");
        let s = row.suggestion.expect("the suggestion rides on the row");
        assert_eq!(s.matched_id, "openai/gpt-oss-120b");
        assert!(near(s.price_in_per_mtok, 0.02) && near(s.price_out_per_mtok, 0.076));
        assert_eq!(s.cache_read, Some(0.018));
        // A later ordinary lookup clears the suggestion.
        crate::record_lookup(&pg, &id, model, "no_match", Some("x"))
            .await
            .unwrap();
        let row = list_unpriced(&pg)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.endpoint == ep)
            .unwrap();
        assert!(row.suggestion.is_none());
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn openrouter_auto_stays_unpriced_and_never_prices_below_zero() {
        let pg = pg().await;
        let ep = name("auto");
        let id = endpoint(&pg, &ep, "openrouter", &["openrouter/auto"]).await;
        // What a release before the fix stored for the live catalog's
        // `openrouter/auto` ("-1" / "-1" per token): -1e6 per mtok.
        published(
            &pg,
            &id,
            "openrouter/auto",
            "",
            -1_000_000.0,
            -1_000_000.0,
            None,
        )
        .await;
        // The review's rows: main's per-turn shape and rc's cut-off stream,
        // 10k in / 1k out each — once -$11 apiece.
        for _ in 0..5 {
            sqlx::query(
                "insert into usage_events (agent_model, source, prompt_tokens, completion_tokens, \
                   estimated, endpoint_class, llm_model, endpoint) \
                 values ($1, 'chat', 10000, 1000, false, 'cloud', 'openrouter/auto', $2)",
            )
            .bind(format!("{ep}:agent"))
            .bind(&ep)
            .execute(&pg)
            .await
            .unwrap();
        }
        let rows = priced_rows(&pg, &ep).await;
        assert_eq!(rows.len(), 5);
        assert!(
            rows.iter().all(|r| r.1.is_none() && r.2.is_none()),
            "a stored -1 is no price: {rows:?}"
        );
        // The budget read sees $0 for them, never a negative sum.
        let window = talaria_gateway::usage::spend_since(&pg, 24, Some(&format!("{ep}:agent")))
            .await
            .unwrap();
        assert_eq!(window.cost, 0.0);
        assert_eq!(window.unpriced_tokens, 5 * 11_000);
        // The oracle's recorded reason reaches the panel.
        crate::record_lookup(
            &pg,
            &id,
            "openrouter/auto",
            "variable",
            Some(&crate::variable_detail("openrouter/auto")),
        )
        .await
        .unwrap();
        let reason = reason_now(&pg, &ep, "openrouter/auto")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reason.code, "variable_price");
        assert!(reason.text.contains("variable price for openrouter/auto"));
        // The provider's own charge still prices a router call.
        book(
            &pg,
            &ep,
            "openrouter/auto",
            tokens(10, 10, 0),
            Some(ReportedSpend {
                cost_usd: Some(0.0042),
                ..ReportedSpend::default()
            }),
        )
        .await;
        let charged = priced_rows(&pg, &ep).await;
        assert!(charged.iter().any(|r| r.2.as_deref() == Some("provider")));
        cleanup(&pg, &ep).await;
    }

    /// PROPERTY: over every combination of the price sources — the row's
    /// class, its provider charge (absent / real / negative), an admin
    /// decision (none / included / a valid manual rate / a negative manual
    /// rate written past the api), published rows (none / real / negative /
    /// a real one with a negative cache rate), the served variant, and token
    /// counts (some written negative) — the view's cost is null or >= 0, and
    /// its basis is `price_basis` on the facts the guards leave standing.
    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn no_combination_of_sources_prices_a_row_below_zero() {
        let pg = pg().await;
        let ep = name("property");
        let id = endpoint(&pg, &ep, "openrouter", &[]).await;
        let classes = [Some("cloud"), Some("local"), None];
        let charges = [None, Some(0.01), Some(-5.0)];
        // (mode, in, out, cache_read)
        type Admin = Option<(&'static str, Option<f64>, Option<f64>, Option<f64>)>;
        let admins: [Admin; 4] = [
            None,
            Some(("included", None, None, None)),
            Some(("manual", Some(2.0), Some(8.0), Some(-1.0))),
            Some(("manual", Some(-2.0), Some(8.0), None)),
        ];
        // (variant, in, out, cache_read, match_kind) rows published for the model.
        type Pub = &'static [(&'static str, f64, f64, Option<f64>, Option<&'static str>)];
        let pubs: [Pub; 6] = [
            &[],
            &[("", 3.0, 15.0, Some(0.3), None)],
            &[("", -1_000_000.0, -1_000_000.0, None, Some("exact"))],
            &[
                ("Anthropic", 3.0, 15.0, Some(-1.0), None),
                ("", -1.0, 2.0, None, Some("exact")),
            ],
            &[
                ("", 3.0, 15.0, None, Some("alias")),
                ("OpenAI", 1.0, 1.0, None, Some("segment")),
            ],
            // Written past every writer: NaN and Infinity never count.
            &[
                ("", f64::NAN, 1.0, Some(f64::INFINITY), None),
                ("Anthropic", 2.0, f64::INFINITY, None, None),
            ],
        ];
        let served = [None, Some("Anthropic"), Some("OpenAI")];
        let counts = [(10_000i64, 1_000i64, 500i64), (-10_000, -5, -1)];
        let mut n = 0;
        for class in classes {
            for charge in charges {
                for admin in admins {
                    for published_rows in pubs {
                        for variant in served {
                            for (prompt, completion, cache_read) in counts {
                                n += 1;
                                let model = format!("m{n}");
                                for (v, inn, out, read, kind) in published_rows {
                                    published(&pg, &id, &model, v, *inn, *out, *read).await;
                                    sqlx::query(
                                        "update provider_prices set match_kind = $3 \
                                         where endpoint_id = $1::uuid and model = $2 and variant = $4",
                                    )
                                    .bind(&id)
                                    .bind(&model)
                                    .bind(*kind)
                                    .bind(*v)
                                    .execute(&pg)
                                    .await
                                    .unwrap();
                                }
                                if let Some((mode, inn, out, read)) = admin {
                                    sqlx::query(
                                        "insert into model_price_overrides (endpoint, model, mode, \
                                           price_in_per_mtok, price_out_per_mtok, price_cache_read_per_mtok) \
                                         values ($1, $2, $3, $4, $5, $6)",
                                    )
                                    .bind(&ep)
                                    .bind(&model)
                                    .bind(mode)
                                    .bind(inn)
                                    .bind(out)
                                    .bind(read)
                                    .execute(&pg)
                                    .await
                                    .unwrap();
                                }
                                sqlx::query(
                                    "insert into usage_events (agent_model, source, prompt_tokens, \
                                       completion_tokens, cache_read_tokens, estimated, endpoint_class, \
                                       llm_model, endpoint, provider_cost, cost_variant) \
                                     values ($1, 'gateway', $2, $3, $4, false, $5, $6, $7, $8, $9)",
                                )
                                .bind(format!("{ep}:caller"))
                                .bind(prompt as i32)
                                .bind(completion as i32)
                                .bind(cache_read as i32)
                                .bind(class)
                                .bind(&model)
                                .bind(&ep)
                                .bind(charge)
                                .bind(variant)
                                .execute(&pg)
                                .await
                                .unwrap();
                                // The facts the guards leave standing.
                                let usable: Vec<(&str, bool)> = published_rows
                                    .iter()
                                    .filter(|(_, inn, out, ..)| {
                                        inn.is_finite()
                                            && out.is_finite()
                                            && *inn >= 0.0
                                            && *out >= 0.0
                                    })
                                    .map(|(v, _, _, _, kind)| {
                                        (*v, kind.is_none_or(|k| k == "exact"))
                                    })
                                    .collect();
                                let admin_fact = match admin {
                                    Some(("included", ..)) => Some(AdminPricing::Included),
                                    Some(("manual", Some(inn), Some(out), _))
                                        if inn >= 0.0 && out >= 0.0 =>
                                    {
                                        Some(AdminPricing::Manual)
                                    }
                                    _ => None,
                                };
                                let want = price_basis(&PriceFacts {
                                    class,
                                    provider_cost: charge.is_some_and(|c| c >= 0.0),
                                    admin: admin_fact,
                                    served_variant: variant,
                                    published: &usable,
                                });
                                let sql = format!(
                                    "with priced as ({}) select cost::float8, cost_basis from priced \
                                     where endpoint = $1 and llm_model = $2",
                                    priced_view()
                                );
                                let (cost, basis): (Option<f64>, Option<String>) =
                                    sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
                                        .bind(&ep)
                                        .bind(&model)
                                        .fetch_one(&pg)
                                        .await
                                        .unwrap();
                                let case = format!(
                                    "{model}: class {class:?}, charge {charge:?}, admin {admin:?}, \
                                     published {published_rows:?}, served {variant:?}, tokens \
                                     ({prompt}, {completion}, {cache_read})"
                                );
                                assert!(
                                    cost.is_none_or(|c| c >= 0.0),
                                    "negative cost {cost:?} — {case}"
                                );
                                assert_eq!(basis.as_deref(), want.map(CostBasis::as_str), "{case}");
                                assert_eq!(cost.is_some(), basis.is_some(), "{case}");
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(n, 3 * 3 * 4 * 6 * 3 * 2);
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_manual_price_reprices_history_until_a_published_price_takes_over() {
        let pg = pg().await;
        let ep = name("manual");
        let id = endpoint(&pg, &ep, "openai", &[]).await;
        // OpenAI's own dated snapshot id; two calls booked by the relay.
        let model = "gpt-4.1-2025-04-14";
        book(&pg, &ep, model, tokens(1_000_000, 100_000, 0), None).await;
        book(&pg, &ep, model, tokens(500_000, 0, 200_000), None).await;

        // Unpriced, and listed with the reason the oracle has (none yet).
        let gap = list_unpriced(&pg)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.endpoint == ep)
            .expect("the model is listed as unpriced");
        assert_eq!(gap.model, model);
        assert_eq!(gap.generations, 2);
        assert_eq!(gap.prompt_tokens, 1_500_000);
        assert_eq!(gap.cache_read_tokens, 200_000);
        assert_eq!(gap.reason.code, "not_checked");

        // The admin's rate: $2 in / $8 out, cache read $0.50, no cache-write
        // rate (the multiplier applies).
        let input = PriceInput {
            endpoint: ep.clone(),
            model: model.into(),
            mode: AdminPricing::Manual,
            price_in_per_mtok: Some(2.0),
            price_out_per_mtok: Some(8.0),
            price_cache_write_per_mtok: None,
            price_cache_read_per_mtok: Some(0.5),
            note: None,
        };
        validate_price(&input).unwrap();
        set_price(&pg, &input, "live-test").await.unwrap();
        // BOTH past rows re-price: 1.0M × 2 + 0.1M × 8 = 2.8; 0.5M × 2 + 0.2M × 0.5 = 1.1.
        let after = repriced(&pg, &ep, model).await.unwrap();
        assert_eq!(
            (after.rows, after.priced_rows, after.unpriced_rows),
            (2, 2, 0)
        );
        assert!(near(after.cost, 3.9), "re-priced cost {}", after.cost);
        assert_eq!(after.basis.as_deref(), Some("manual"));
        assert!(
            list_unpriced(&pg)
                .await
                .unwrap()
                .iter()
                .all(|m| m.endpoint != ep),
            "a priced model leaves the list"
        );
        let setting = list_settings(&pg)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.endpoint == ep)
            .expect("the setting is listed");
        assert_eq!(
            (setting.mode, setting.rows_priced, setting.shadowed),
            ("manual", 2, false)
        );

        // A published price lands later: it beats the manual rate (manual is
        // the fallback), and the setting says it is shadowed.
        published(&pg, &id, model, "", 3.0, 12.0, None).await;
        let rows = priced_rows(&pg, &ep).await;
        assert!(rows.iter().all(|r| r.2.as_deref() == Some("derived")));
        let setting = list_settings(&pg)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.endpoint == ep)
            .unwrap();
        assert_eq!((setting.rows_priced, setting.shadowed), (0, true));
        // Removing it leaves the published price in charge.
        assert!(clear_price(&pg, &ep, model).await.unwrap());
        assert!(
            !clear_price(&pg, &ep, model).await.unwrap(),
            "a second remove finds none"
        );
        assert_eq!(
            repriced(&pg, &ep, model).await.unwrap().basis.as_deref(),
            Some("derived")
        );
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn included_is_zero_labelled_and_beats_a_published_price_but_not_the_charge() {
        let pg = pg().await;
        let ep = name("included");
        let id = endpoint(&pg, &ep, "custom", &["glm-4.6"]).await;
        // A Z.ai Coding Plan call: usage with no cost (observed), and the
        // catalog's pay-as-you-go GLM price matched for it.
        published(&pg, &id, "glm-4.6", "", 0.5, 2.0, Some(0.1)).await;
        book(&pg, &ep, "glm-4.6", tokens(19, 16, 0), None).await;
        // A second call that DID carry a charge.
        book(
            &pg,
            &ep,
            "glm-4.6",
            tokens(10, 10, 0),
            Some(ReportedSpend {
                cost_usd: Some(0.0042),
                ..ReportedSpend::default()
            }),
        )
        .await;
        assert_eq!(
            priced_rows(&pg, &ep)
                .await
                .iter()
                .map(|r| r.2.clone())
                .collect::<Vec<_>>(),
            [Some("derived".into()), Some("provider".into())],
            "before: the plan call is priced at pay-as-you-go rates"
        );
        set_price(
            &pg,
            &PriceInput {
                endpoint: ep.clone(),
                model: "glm-4.6".into(),
                mode: AdminPricing::Included,
                price_in_per_mtok: None,
                price_out_per_mtok: None,
                price_cache_write_per_mtok: None,
                price_cache_read_per_mtok: None,
                note: Some("Z.ai Coding Plan".into()),
            },
            "live-test",
        )
        .await
        .unwrap();
        let rows = priced_rows(&pg, &ep).await;
        assert_eq!(rows[0].1, Some(0.0));
        assert_eq!(
            rows[0].2.as_deref(),
            Some("included"),
            "$0 says why — never a silent zero"
        );
        assert!(
            near(rows[1].1.unwrap(), 0.0042),
            "the provider's own charge still wins"
        );
        assert_eq!(rows[1].2.as_deref(), Some("provider"));
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn openrouter_rows_without_a_serving_variant_price_from_the_model_level_row() {
        let pg = pg().await;
        let ep = name("openrouter");
        let id = endpoint(&pg, &ep, "openrouter", &["anthropic/claude-sonnet-4.5"]).await;
        let model = "anthropic/claude-sonnet-4.5";
        // What the oracle stores: OpenRouter's /models price (variant '') and
        // two per-endpoint prices from /endpoints.
        published(&pg, &id, model, "", 3.0, 15.0, Some(0.3)).await;
        published(&pg, &id, model, "Anthropic", 3.0, 15.0, Some(0.3)).await;
        published(&pg, &id, model, "Amazon Bedrock", 3.3, 16.5, Some(0.33)).await;
        // (1) main's per-turn agent row, as gateway::usage::record_usage wrote
        //     it on 805e6799: no charge, no variant, no generation id.
        sqlx::query(
            "insert into usage_events (agent_model, source, prompt_tokens, completion_tokens, \
               cache_write_tokens, cache_read_tokens, reasoning_tokens, estimated, \
               endpoint_class, llm_model, endpoint) \
             values ($1, 'chat', 1000000, 0, 0, 1000000, 0, false, 'cloud', $2, $3)",
        )
        .bind(format!("{ep}:agent"))
        .bind(model)
        .bind(&ep)
        .execute(&pg)
        .await
        .unwrap();
        // (2) rc's relay row for a stream cut off before its usage chunk: no
        //     cost, a generation id, no variant.
        record_relay_usage(
            &pg,
            UNATTENDED_SOURCE,
            &format!("{ep}:agent"),
            None,
            &StatedFlow::default(),
            &ep,
            "cloud",
            model,
            &tokens(1_000_000, 0, 0),
            true,
            Some(&ReportedSpend {
                generation_id: Some("gen-live-1".into()),
                provider: Some("openrouter".into()),
                ..ReportedSpend::default()
            }),
        )
        .await
        .unwrap();
        // (3) a relay row that named its serving endpoint and carried no cost.
        book(
            &pg,
            &ep,
            model,
            tokens(1_000_000, 0, 0),
            Some(ReportedSpend {
                variant: Some("Amazon Bedrock".into()),
                ..ReportedSpend::default()
            }),
        )
        .await;
        let rows = priced_rows(&pg, &ep).await;
        // (1) 1M × $3 + 1M cache-read × the published $0.30 = 3.3.
        assert!(
            near(rows[0].1.expect("main's row is priced now"), 3.3),
            "{rows:?}"
        );
        // (2) the model-level price.
        assert!(near(
            rows[1].1.expect("the cut-off stream is priced now"),
            3.0
        ));
        // (3) the exact variant beats the model-level row.
        assert!(near(rows[2].1.expect("the variant row is priced"), 3.3));
        assert!(rows.iter().all(|r| r.2.as_deref() == Some("derived")));
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn the_sql_decision_agrees_with_price_basis_on_every_case() {
        let pg = pg().await;
        let ep = name("parity");
        let id = endpoint(&pg, &ep, "openai", &[]).await;
        // (model, provider charge?, admin decision, published variants, served variant)
        type Case = (
            &'static str,
            bool,
            Option<AdminPricing>,
            &'static [&'static str],
            Option<&'static str>,
        );
        let cases: [Case; 9] = [
            ("m-nothing", false, None, &[], None),
            ("m-charged", true, Some(AdminPricing::Included), &[""], None),
            (
                "m-included",
                false,
                Some(AdminPricing::Included),
                &[""],
                None,
            ),
            (
                "m-published",
                false,
                Some(AdminPricing::Manual),
                &[""],
                None,
            ),
            ("m-manual", false, Some(AdminPricing::Manual), &[], None),
            (
                "m-variant",
                false,
                None,
                &["Infermatic"],
                Some("Infermatic"),
            ),
            (
                "m-other-variant",
                false,
                None,
                &["Infermatic"],
                Some("OpenAI"),
            ),
            ("m-variant-falls-back", false, None, &[""], Some("OpenAI")),
            (
                "m-included-only",
                false,
                Some(AdminPricing::Included),
                &[],
                None,
            ),
        ];
        for (model, charged, admin, variants, served) in cases {
            for v in variants {
                published(&pg, &id, model, v, 1.0, 2.0, None).await;
            }
            if let Some(mode) = admin {
                let (inn, out) = match mode {
                    AdminPricing::Manual => (Some(5.0), Some(6.0)),
                    AdminPricing::Included => (None, None),
                };
                set_price(
                    &pg,
                    &PriceInput {
                        endpoint: ep.clone(),
                        model: model.into(),
                        mode,
                        price_in_per_mtok: inn,
                        price_out_per_mtok: out,
                        price_cache_write_per_mtok: None,
                        price_cache_read_per_mtok: None,
                        note: None,
                    },
                    "live-test",
                )
                .await
                .unwrap();
            }
            book(
                &pg,
                &ep,
                model,
                tokens(1000, 1000, 0),
                Some(ReportedSpend {
                    cost_usd: charged.then_some(0.01),
                    variant: served.map(str::to_string),
                    ..ReportedSpend::default()
                }),
            )
            .await;
            let rows = priced_rows(&pg, &ep).await;
            let (_, _, sql_basis) = rows.iter().find(|r| r.0 == model).expect("booked");
            let exact: Vec<(&str, bool)> = variants.iter().map(|v| (*v, true)).collect();
            let rust = price_basis(&PriceFacts {
                class: Some("cloud"),
                provider_cost: charged,
                admin,
                served_variant: served,
                published: &exact,
            });
            assert_eq!(
                sql_basis.as_deref(),
                rust.map(CostBasis::as_str),
                "{model}: the view and price_basis disagree"
            );
        }
        cleanup(&pg, &ep).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_source_that_did_not_answer_is_the_reason_quoted_and_a_miss_names_what_was_tried() {
        let pg = pg().await;
        let ep = name("reasons");
        let id = endpoint(
            &pg,
            &ep,
            "custom",
            &["llama-3.3-70b-versatile", "priced-already"],
        )
        .await;
        published(&pg, &id, "priced-already", "", 1.0, 1.0, None).await;
        book(&pg, &ep, "llama-3.3-70b-versatile", tokens(10, 10, 0), None).await;
        let targets = crate::models_to_price(&pg, Some((ep.as_str(), "llama-3.3-70b-versatile")))
            .await
            .unwrap();
        assert_eq!(targets.len(), 1);
        // The catalog fetch failed: the reason is the source's own words, on
        // the model with no price to fall back on, and only on it.
        crate::record_source_failure(
            &pg,
            &crate::models_to_price(&pg, None)
                .await
                .unwrap()
                .into_iter()
                .filter(|t| t.1 == ep)
                .collect::<Vec<_>>(),
            "openrouter answered 503: upstream connect error",
        )
        .await;
        let reason = reason_now(&pg, &ep, "llama-3.3-70b-versatile")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reason.code, "source_failed");
        assert_eq!(
            reason.text,
            "The price source didn't answer: openrouter answered 503: upstream connect error"
        );
        assert!(reason.checked_at.is_some());
        let skipped: Option<(String,)> = sqlx::query_as(
            "select outcome from price_lookups where endpoint_id = $1::uuid and model = 'priced-already'",
        )
        .bind(&id)
        .fetch_optional(&pg)
        .await
        .unwrap();
        assert_eq!(skipped, None, "a priced model is not marked failed");
        // A miss: the recorded detail names the catalog and every id tried.
        crate::record_lookup(
            &pg,
            &id,
            "llama-3.3-70b-versatile",
            "no_match",
            Some(&crate::no_match_detail("llama-3.3-70b-versatile", 454)),
        )
        .await
        .unwrap();
        let reason = reason_now(&pg, &ep, "llama-3.3-70b-versatile")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reason.code, "no_match");
        assert!(reason.text.contains("454 priced models"));
        cleanup(&pg, &ep).await;
    }
}

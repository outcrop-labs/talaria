//! Fetch published prices and provider activity, and backfill generation costs.
//! A provider that answers 401/403 is skipped — that is the graceful degrade,
//! not a guessed price.

use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;

use crate::spend::{
    ActivityRow, PublishedPrice, parse_anthropic_costs, parse_openai_costs,
    parse_openrouter_activity, parse_openrouter_credits, parse_openrouter_endpoints,
    parse_openrouter_generation, parse_provider_models,
};

const TIMEOUT: Duration = Duration::from_secs(15);

struct EndpointRow {
    id: String,
    provider: String,
    base_url: Option<String>,
    models: Value,
}

pub struct KeyedEndpoint {
    pub id: String,
    pub provider: String,
    pub key: String,
}

async fn get_json(url: &str, key: Option<&str>) -> Result<Value, String> {
    let mut req = talaria_gateway::provider::http().get(url);
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    let resp = tokio::time::timeout(TIMEOUT, req.send())
        .await
        .map_err(|_| format!("timeout {url}"))?
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{} {}", url, resp.status().as_u16()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

fn model_ids(models: &Value) -> Vec<String> {
    models
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Upsert one published price. The unique key is (endpoint_id, model, variant),
/// so a model-level row (variant '') and each upstream variant coexist.
pub(crate) async fn upsert_price(
    pg: &PgPool,
    endpoint_id: &str,
    provider: &str,
    price: &PublishedPrice,
) -> Result<(), String> {
    sqlx::query(
        "insert into provider_prices (endpoint_id, provider, model, variant, price_in_per_mtok, \
         price_out_per_mtok, source, fetched_at) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7, now()) \
         on conflict (endpoint_id, model, variant) do update set \
           price_in_per_mtok = excluded.price_in_per_mtok, \
           price_out_per_mtok = excluded.price_out_per_mtok, \
           source = excluded.source, \
           fetched_at = now()",
    )
    .bind(endpoint_id)
    .bind(provider)
    .bind(&price.model)
    .bind(&price.variant)
    .bind(price.price_in_per_mtok)
    .bind(price.price_out_per_mtok)
    .bind(price.source)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn upsert_activity(
    pg: &PgPool,
    endpoint_id: &str,
    provider: &str,
    source: &str,
    row: &ActivityRow,
) -> Result<(), String> {
    sqlx::query(
        "insert into provider_spend (endpoint_id, provider, variant, model, window_start, window_end, \
         cost_usd, prompt_tokens, completion_tokens, requests, source, fetched_at) \
         values ($1::uuid, $2, $3, $4, $5::date, ($5::date + interval '1 day'), $6, $7, $8, $9, $10, now()) \
         on conflict (endpoint_id, source, window_start, model, variant) do update set \
           cost_usd = excluded.cost_usd, \
           prompt_tokens = excluded.prompt_tokens, \
           completion_tokens = excluded.completion_tokens, \
           requests = excluded.requests, \
           fetched_at = now()",
    )
    .bind(endpoint_id)
    .bind(provider)
    .bind(&row.variant)
    .bind(&row.model)
    .bind(&row.date)
    .bind(row.cost_usd)
    .bind(row.prompt_tokens)
    .bind(row.completion_tokens)
    .bind(row.requests)
    .bind(source)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Published prices. OpenRouter is per-endpoint. Other providers contribute
/// only what their own `/models` payload prices. A refusal is a skip.
pub async fn ingest_published_prices(pg: &PgPool) -> Result<usize, String> {
    let endpoints: Vec<(String, String, Option<String>, Value)> = sqlx::query_as(
        "select id::text, provider, base_url, models from llm_endpoints where class = 'cloud'",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let mut n = 0;
    for (id, provider, base_url, models) in endpoints {
        let row = EndpointRow {
            id,
            provider,
            base_url,
            models,
        };
        n += ingest_one_catalog(pg, &row).await;
    }
    talaria_gateway::registry::invalidate_endpoints_cache();
    Ok(n)
}

async fn ingest_one_catalog(pg: &PgPool, ep: &EndpointRow) -> usize {
    let mut n = 0;
    if ep.provider == "openrouter" {
        for model in model_ids(&ep.models) {
            let Some((author, slug)) = model.split_once('/') else {
                continue;
            };
            let url = format!("https://openrouter.ai/api/v1/models/{author}/{slug}/endpoints");
            let Ok(body) = get_json(&url, None).await else {
                continue;
            };
            for price in parse_openrouter_endpoints(&model, &body) {
                if upsert_price(pg, &ep.id, &ep.provider, &price).await.is_ok() {
                    n += 1;
                }
            }
        }
        return n;
    }
    let Some(base) = ep.base_url.as_deref() else {
        return 0;
    };
    let url = format!("{}/models", base.trim_end_matches('/'));
    let Ok(body) = get_json(&url, None).await else {
        return 0;
    };
    for price in parse_provider_models(&body) {
        if upsert_price(pg, &ep.id, &ep.provider, &price).await.is_ok() {
            n += 1;
        }
    }
    n
}

/// Activity / cost-report / credits, plus OpenRouter generation backfill.
/// A 401 or 403 skips that provider — no invented number.
pub async fn ingest_provider_spend(pg: &PgPool, keyed: &[KeyedEndpoint]) -> Result<usize, String> {
    let mut n = 0;
    for ep in keyed {
        match ep.provider.as_str() {
            "openrouter" => n += ingest_openrouter(pg, ep).await,
            "openai" => {
                n += ingest_window(
                    pg,
                    ep,
                    "https://api.openai.com/v1/organization/costs?bucket_width=1d&limit=31",
                    "openai.costs",
                    parse_openai_costs,
                )
                .await
            }
            "anthropic" => {
                n += ingest_window(
                    pg,
                    ep,
                    "https://api.anthropic.com/v1/organizations/cost_report?bucket_width=1d",
                    "anthropic.cost_report",
                    parse_anthropic_costs,
                )
                .await;
            }
            _ => {}
        }
    }
    Ok(n)
}

async fn ingest_openrouter(pg: &PgPool, ep: &KeyedEndpoint) -> usize {
    let mut n = 0;
    if let Ok(body) = get_json("https://openrouter.ai/api/v1/activity", Some(&ep.key)).await {
        for row in parse_openrouter_activity(&body) {
            if upsert_activity(pg, &ep.id, "openrouter", "openrouter.activity", &row)
                .await
                .is_ok()
            {
                n += 1;
            }
        }
    }
    if let Ok(body) = get_json("https://openrouter.ai/api/v1/credits", Some(&ep.key)).await
        && let Some(snap) = parse_openrouter_credits(&body)
    {
        let row = ActivityRow {
            model: String::new(),
            variant: String::new(),
            cost_usd: snap.total_usage,
            prompt_tokens: None,
            completion_tokens: None,
            requests: None,
            date: "1970-01-01".into(),
        };
        if upsert_activity(pg, &ep.id, "openrouter", "openrouter.credits", &row)
            .await
            .is_ok()
        {
            n += 1;
        }
    }
    n += backfill_generations(pg, ep).await;
    n
}

async fn backfill_generations(pg: &PgPool, ep: &KeyedEndpoint) -> usize {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select id::text, generation_id from usage_events \
         where endpoint = (select name from llm_endpoints where id = $1::uuid) \
           and provider_cost is null and generation_id is not null \
           and created_at > now() - interval '2 days' \
         limit 40",
    )
    .bind(&ep.id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    let mut n = 0;
    for (id, generation_id) in rows {
        let url = format!("https://openrouter.ai/api/v1/generation?id={generation_id}");
        let Ok(body) = get_json(&url, Some(&ep.key)).await else {
            continue;
        };
        let Some((cost, variant)) = parse_openrouter_generation(&body) else {
            continue;
        };
        if sqlx::query(
            "update usage_events set provider_cost = $2, cost_source = 'provider', \
             cost_provider = 'openrouter', cost_variant = coalesce($3, cost_variant), \
             cost_fetched_at = now() \
             where id = $1::uuid and provider_cost is null",
        )
        .bind(&id)
        .bind(cost)
        .bind(variant)
        .execute(pg)
        .await
        .is_ok()
        {
            n += 1;
        }
    }
    n
}

async fn ingest_window(
    pg: &PgPool,
    ep: &KeyedEndpoint,
    url: &str,
    source: &str,
    parse: fn(&Value) -> Vec<ActivityRow>,
) -> usize {
    let Ok(body) = get_json(url, Some(&ep.key)).await else {
        return 0;
    };
    let mut n = 0;
    for row in parse(&body) {
        if upsert_activity(pg, &ep.id, &ep.provider, source, &row)
            .await
            .is_ok()
        {
            n += 1;
        }
    }
    n
}

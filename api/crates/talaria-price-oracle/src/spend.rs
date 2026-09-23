//! Provider-reported spend and published prices.
//!
//! Spend is the provider's number when they report one (a completion's
//! `usage.cost`, a generation lookup, or an activity/cost-report window).
//! A published price is a labeled fallback, never an admin-typed rate, and
//! OpenRouter's per-endpoint prices are stored as separate rows — not averaged.

use serde_json::Value;

/// One activity (or cost-report) row, already in dollars.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityRow {
    pub model: String,
    pub variant: String,
    pub cost_usd: f64,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub requests: Option<i32>,
    /// UTC date `YYYY-MM-DD` the provider attributed the spend to.
    pub date: String,
}

/// One published price. `variant` empty means a model-level rate, not an
/// upstream endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedPrice {
    pub model: String,
    pub variant: String,
    pub price_in_per_mtok: Option<f64>,
    pub price_out_per_mtok: Option<f64>,
    pub source: &'static str,
}

/// Account credit snapshot. Cumulative usage, not a window.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditSnapshot {
    pub total_credits: Option<f64>,
    pub total_usage: f64,
}

fn f64_of(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite() && *n >= 0.0)
}

fn i64_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// OpenRouter `GET /activity` — one row per (date, model, endpoint). Prices
/// are not combined across endpoints.
pub fn parse_openrouter_activity(body: &Value) -> Vec<ActivityRow> {
    let Some(rows) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let cost = f64_of(row.get("usage"))?;
            let date = row.get("date")?.as_str()?.to_string();
            if date.len() != 10 {
                return None;
            }
            Some(ActivityRow {
                model: row
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                variant: row
                    .get("endpoint_id")
                    .and_then(Value::as_str)
                    .or_else(|| row.get("provider_name").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string(),
                cost_usd: cost,
                prompt_tokens: i64_of(row.get("prompt_tokens")),
                completion_tokens: i64_of(row.get("completion_tokens")),
                requests: i64_of(row.get("requests")).and_then(|n| i32::try_from(n).ok()),
                date,
            })
        })
        .collect()
}

/// OpenRouter `GET /models/{author}/{slug}/endpoints`. Each endpoint keeps its
/// own price. Per-token strings are scaled to $/MTok.
pub fn parse_openrouter_endpoints(model: &str, body: &Value) -> Vec<PublishedPrice> {
    let endpoints = body
        .get("data")
        .and_then(|d| d.get("endpoints"))
        .and_then(Value::as_array);
    let Some(endpoints) = endpoints else {
        return Vec::new();
    };
    endpoints
        .iter()
        .filter_map(|ep| {
            let pricing = ep.get("pricing")?;
            let inn = f64_of(pricing.get("prompt")).map(|p| p * 1_000_000.0);
            let out = f64_of(pricing.get("completion")).map(|p| p * 1_000_000.0);
            if inn.is_none() && out.is_none() {
                return None;
            }
            let variant = ep
                .get("provider_name")
                .and_then(Value::as_str)
                .or_else(|| ep.get("tag").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            if variant.is_empty() {
                return None;
            }
            Some(PublishedPrice {
                model: model.to_string(),
                variant,
                price_in_per_mtok: inn,
                price_out_per_mtok: out,
                source: "openrouter.endpoints",
            })
        })
        .collect()
}

/// A provider `/models` payload that includes `pricing.prompt` / `completion`
/// (per-token). Stored as a model-level published price. Not used for
/// OpenRouter — that provider's charge depends on the endpoint.
pub fn parse_provider_models(body: &Value) -> Vec<PublishedPrice> {
    let Some(rows) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            let pricing = row.get("pricing")?;
            let inn = f64_of(pricing.get("prompt")).map(|p| p * 1_000_000.0);
            let out = f64_of(pricing.get("completion")).map(|p| p * 1_000_000.0);
            if inn.is_none() && out.is_none() {
                return None;
            }
            Some(PublishedPrice {
                model: id.to_string(),
                variant: String::new(),
                price_in_per_mtok: inn,
                price_out_per_mtok: out,
                source: "provider.models",
            })
        })
        .collect()
}

pub fn parse_openrouter_credits(body: &Value) -> Option<CreditSnapshot> {
    let data = body.get("data").unwrap_or(body);
    let total_usage = f64_of(data.get("total_usage"))?;
    Some(CreditSnapshot {
        total_credits: f64_of(data.get("total_credits")),
        total_usage,
    })
}

/// `GET /generation?id=` — the provider's cost for that call, plus the
/// upstream endpoint (`provider_name`) that served it.
pub fn parse_openrouter_generation(body: &Value) -> Option<(f64, Option<String>)> {
    let data = body.get("data").unwrap_or(body);
    let cost = f64_of(data.get("total_cost")).or_else(|| f64_of(data.get("usage")))?;
    let variant = data
        .get("provider_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((cost, variant))
}

/// OpenAI `GET /v1/organization/costs`. Amounts are already dollars.
pub fn parse_openai_costs(body: &Value) -> Vec<ActivityRow> {
    let Some(buckets) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for bucket in buckets {
        let Some(start) = bucket.get("start_time").and_then(Value::as_i64) else {
            continue;
        };
        let date = unix_date(start);
        let Some(results) = bucket.get("results").and_then(Value::as_array) else {
            continue;
        };
        for result in results {
            let amount = result.get("amount");
            let cost = f64_of(amount.and_then(|a| a.get("value"))).or_else(|| f64_of(amount));
            let Some(cost) = cost else { continue };
            out.push(ActivityRow {
                model: result
                    .get("line_item")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                variant: result
                    .get("project_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                cost_usd: cost,
                prompt_tokens: None,
                completion_tokens: None,
                requests: None,
                date: date.clone(),
            });
        }
    }
    out
}

/// Anthropic `GET /v1/organizations/cost_report`. `amount` is a dollar string.
pub fn parse_anthropic_costs(body: &Value) -> Vec<ActivityRow> {
    let Some(buckets) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for bucket in buckets {
        let Some(start) = bucket.get("starting_at").and_then(Value::as_str) else {
            continue;
        };
        let date = start.chars().take(10).collect::<String>();
        if date.len() != 10 {
            continue;
        }
        let Some(results) = bucket.get("results").and_then(Value::as_array) else {
            continue;
        };
        for result in results {
            let Some(cost) = f64_of(result.get("amount")) else {
                continue;
            };
            out.push(ActivityRow {
                model: result
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                variant: result
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                cost_usd: cost,
                prompt_tokens: None,
                completion_tokens: None,
                requests: None,
                date: date.clone(),
            });
        }
    }
    out
}

fn unix_date(secs: i64) -> String {
    // Civil date from Unix seconds, UTC. Howard Hinnant's algorithm.
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
pub fn activity_total(rows: &[ActivityRow]) -> f64 {
    rows.iter().map(|r| r.cost_usd).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openrouter_activity_keeps_each_endpoint() {
        let body = json!({
            "data": [
                {
                    "date": "2026-09-22",
                    "model": "anthropic/claude-sonnet-4",
                    "endpoint_id": "ep-a",
                    "provider_name": "Anthropic",
                    "usage": 0.015,
                    "prompt_tokens": 50,
                    "completion_tokens": 125,
                    "requests": 5
                },
                {
                    "date": "2026-09-22",
                    "model": "anthropic/claude-sonnet-4",
                    "endpoint_id": "ep-b",
                    "provider_name": "Amazon Bedrock",
                    "usage": 0.040,
                    "requests": 1
                }
            ]
        });
        let rows = parse_openrouter_activity(&body);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].variant, "ep-a");
        assert_eq!(rows[1].variant, "ep-b");
        assert!((activity_total(&rows) - 0.055).abs() < 1e-12);
        // Not an average of the two endpoint charges.
        assert!((rows[0].cost_usd - 0.015).abs() < 1e-12);
    }

    #[test]
    fn openrouter_endpoints_are_not_averaged() {
        let body = json!({
            "data": {
                "id": "openai/gpt-4",
                "endpoints": [
                    {
                        "provider_name": "OpenAI",
                        "pricing": { "prompt": "0.00003", "completion": "0.00006" }
                    },
                    {
                        "provider_name": "Azure",
                        "pricing": { "prompt": "0.00001", "completion": "0.00002" }
                    }
                ]
            }
        });
        let prices = parse_openrouter_endpoints("openai/gpt-4", &body);
        assert_eq!(prices.len(), 2);
        assert_eq!(prices[0].variant, "OpenAI");
        assert_eq!(prices[0].price_in_per_mtok, Some(30.0));
        assert_eq!(prices[1].price_in_per_mtok, Some(10.0));
        assert_eq!(prices[0].source, "openrouter.endpoints");
    }

    #[test]
    fn generation_lookup_returns_the_serving_endpoint() {
        let body = json!({
            "data": { "total_cost": 0.0015, "provider_name": "Infermatic", "usage": 0.0015 }
        });
        let (cost, variant) = parse_openrouter_generation(&body).unwrap();
        assert!((cost - 0.0015).abs() < 1e-12);
        assert_eq!(variant.as_deref(), Some("Infermatic"));
    }

    #[test]
    fn credits_are_a_balance_not_a_per_call_price() {
        let snap = parse_openrouter_credits(&json!({
            "data": { "total_credits": 100.0, "total_usage": 25.5 }
        }))
        .unwrap();
        assert_eq!(snap.total_usage, 25.5);
        assert_eq!(snap.total_credits, Some(100.0));
    }

    #[test]
    fn openai_and_anthropic_cost_reports_parse() {
        let openai = parse_openai_costs(&json!({
            "data": [{
                "start_time": 1_758_499_200,
                "results": [{ "amount": { "value": 1.25, "currency": "usd" }, "line_item": "gpt-4o" }]
            }]
        }));
        assert_eq!(openai.len(), 1);
        assert!((openai[0].cost_usd - 1.25).abs() < 1e-12);
        assert_eq!(openai[0].date.len(), 10);

        let anthropic = parse_anthropic_costs(&json!({
            "data": [{
                "starting_at": "2026-09-22T00:00:00Z",
                "results": [{ "amount": "0.42", "model": "claude-sonnet-4" }]
            }]
        }));
        assert_eq!(anthropic.len(), 1);
        assert!((anthropic[0].cost_usd - 0.42).abs() < 1e-12);
        assert_eq!(anthropic[0].model, "claude-sonnet-4");
    }

    #[test]
    fn provider_models_without_pricing_stay_empty() {
        let body = json!({ "data": [{ "id": "gpt-4o" }] });
        assert!(parse_provider_models(&body).is_empty());
    }
}

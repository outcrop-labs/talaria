// Token ledger — port of ui/src/server/usage.ts: usage normalization across
// every provider shape, the chars/4 estimate, the priced rolling-window spend
// read (budget check's read side), and the gateway's usage_events insert.

use serde_json::Value;
use sqlx::PgPool;

pub const CACHE_WRITE_MULTIPLIER: f64 = 1.25;
pub const CACHE_READ_MULTIPLIER: f64 = 0.1;

/// Priced token counts: `prompt_tokens` is UNCACHED input only, so the four
/// fields never overlap and each is billed at its own rate.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TokenCounts {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    /// Informational — already inside completion_tokens, never re-priced.
    pub reasoning_tokens: i64,
}

/// Read a number the way TS's `n()` does: only a JSON number counts (a string
/// digit would be 0 in TS, not parsed), clamped to non-negative integers.
fn num(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) if n.as_f64().is_some_and(|f| f.is_finite()) => {
            n.as_f64().unwrap().max(0.0).round() as i64
        }
        _ => 0,
    }
}

fn at<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|x| !x.is_null())
}

/// Normalise a provider usage object into non-overlapping, separately-priced
/// counts. Port of normalizeUsage — including the shape detection from the
/// payload (not the provider name) and the KNOWN GAP on folded cache writes;
/// see usage.ts:67 for the full rationale.
pub fn normalize_usage(u: Option<&Value>) -> Option<TokenCounts> {
    let u = u?;
    if !u.is_object() {
        return None;
    }
    let mut prompt_tokens = num(at(u, "prompt_tokens").or_else(|| at(u, "input_tokens")));
    let completion_tokens = num(at(u, "completion_tokens").or_else(|| at(u, "output_tokens")));
    let reasoning_tokens =
        num(at(u, "completion_tokens_details").and_then(|d| at(d, "reasoning_tokens")))
            .min(completion_tokens);

    let details = at(u, "prompt_tokens_details");
    let mut cache_write_tokens = num(at(u, "cache_creation_input_tokens")
        .or_else(|| details.and_then(|d| at(d, "cache_creation_tokens"))));
    let mut cache_read_tokens = num(at(u, "cache_read_input_tokens"));
    if cache_write_tokens == 0 && cache_read_tokens == 0 {
        // OpenAI-compatible: cached input is already counted inside
        // prompt_tokens, so bill it once — at the cache-read rate.
        cache_read_tokens = num(details.and_then(|d| at(d, "cached_tokens"))).min(prompt_tokens);
        prompt_tokens -= cache_read_tokens;
    } else if at(u, "cache_read_input_tokens").is_none()
        && at(u, "cache_creation_input_tokens").is_none()
    {
        // Detail-object cache writes are folded in (the KNOWN GAP in usage.ts).
        cache_write_tokens = cache_write_tokens.min(prompt_tokens);
        prompt_tokens -= cache_write_tokens;
    }
    let c = TokenCounts {
        prompt_tokens,
        completion_tokens,
        cache_write_tokens,
        cache_read_tokens,
        reasoning_tokens,
    };
    if c.prompt_tokens + c.completion_tokens + c.cache_write_tokens + c.cache_read_tokens == 0 {
        return None;
    }
    Some(c)
}

/// Rough token estimate when the gateway doesn't report usage.
pub fn estimate_tokens(chars: usize) -> i64 {
    (chars as f64 / 4.0).ceil() as i64
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SpendWindow {
    pub tokens: i64,
    /// Priced spend in USD. Unpriced cloud rows contribute 0 — a $ ceiling is
    /// never tripped by a model whose rate isn't configured.
    pub cost: f64,
    pub unpriced_tokens: i64,
}

// The priced view, verbatim from usage.ts: cloud rows get $ from the user's
// per-model override, else the auto-fetched public rate, else the endpoint
// default; local rows are $0; cloud rows with no price at all get NULL cost
// so they can be surfaced as "unpriced". Multipliers per input KIND.
const PRICED: &str = "select u.*, \
    case \
      when u.endpoint_class = 'local' then 0 \
      when u.endpoint_class = 'cloud' then \
        ((u.prompt_tokens + u.cache_write_tokens * $2 + u.cache_read_tokens * $3) \
           * coalesce((e.model_prices->u.llm_model->>'in')::numeric, \
                      (e.auto_prices->u.llm_model->>'in')::numeric, e.price_in_per_mtok) \
         + u.completion_tokens * coalesce((e.model_prices->u.llm_model->>'out')::numeric, \
                                          (e.auto_prices->u.llm_model->>'out')::numeric, e.price_out_per_mtok)) / 1e6 \
      else null \
    end as cost \
  from usage_events u \
  left join llm_endpoints e on e.name = u.endpoint";

/// Billable spend over a rolling window, optionally for one caller
/// (`agent_model` — a fleet model id for persona rows, `api:<key>` for
/// gateway rows). Port of spendSince; called before every budgeted call and
/// cached by budget.rs.
pub async fn spend_since(
    pg: &PgPool,
    window_hours: i64,
    agent_model: Option<&str>,
) -> Result<SpendWindow, sqlx::Error> {
    let hours = window_hours.clamp(1, 24 * 365);
    // sqlx can't interpolate the interval literal as a bind for `now() - $1`
    // with unit attached — build `make_interval(hours => $1)` instead, which
    // stays a bound parameter the way TS's unsafe string does not.
    let sql = format!(
        "with priced as ({PRICED}) \
         select coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens), 0)::bigint as tokens, \
                coalesce(sum(cost), 0)::float8 as cost, \
                coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where cost is null), 0)::bigint as unpriced \
         from priced \
         where created_at > now() - make_interval(hours => $1) \
           and ($2::text is null or agent_model = $2)"
    );
    // AssertSqlSafe: the only interpolation is the PRICED const above — no
    // caller text reaches this string, and both runtime values stay binds.
    let row: Option<(i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(hours)
        .bind(agent_model)
        .fetch_optional(pg)
        .await?;
    Ok(row
        .map(|(tokens, cost, unpriced)| SpendWindow {
            tokens,
            cost,
            unpriced_tokens: unpriced,
        })
        .unwrap_or_default())
}

/// Ledger row for a gateway call — port of recordGatewayUsage. Attribution is
/// direct (we KNOW the endpoint), no agent-def classification involved.
/// `k()` semantics from TS: non-finite/negative → 0, rounded.
fn k(v: i64) -> i64 {
    v.max(0)
}

#[allow(clippy::too_many_arguments)]
pub async fn record_gateway_usage(
    pg: &PgPool,
    caller: &str,
    endpoint_name: &str,
    endpoint_class: &str,
    upstream_model: &str,
    counts: &TokenCounts,
    estimated: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into usage_events (agent_model, source, prompt_tokens, completion_tokens, \
                                  cache_write_tokens, cache_read_tokens, reasoning_tokens, \
                                  estimated, endpoint_class, llm_model, endpoint) \
         values ($1, 'gateway', $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(caller)
    .bind(k(counts.prompt_tokens))
    .bind(k(counts.completion_tokens))
    .bind(k(counts.cache_write_tokens))
    .bind(k(counts.cache_read_tokens))
    .bind(k(counts.reasoning_tokens))
    .bind(estimated)
    .bind(endpoint_class)
    .bind(upstream_model)
    .bind(endpoint_name)
    .execute(pg)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn norm(v: Value) -> Option<TokenCounts> {
        normalize_usage(Some(&v))
    }

    #[test]
    fn openai_shape_folds_cached_reads_out_of_prompt() {
        // prompt_tokens INCLUDES cached_tokens (OpenAI-compatible): the cached
        // 100 moves to the cache-read bucket, fresh input bills the rest.
        let c = norm(json!({"prompt_tokens": 500, "completion_tokens": 40,
                            "prompt_tokens_details": {"cached_tokens": 100}}))
        .unwrap();
        assert_eq!(c.prompt_tokens, 400);
        assert_eq!(c.cache_read_tokens, 100);
        assert_eq!(c.completion_tokens, 40);
    }

    #[test]
    fn anthropic_native_shape_bills_cache_on_top() {
        // input_tokens EXCLUDES the cache fields — nothing is folded.
        let c = norm(json!({"input_tokens": 300, "output_tokens": 20,
                            "cache_creation_input_tokens": 50, "cache_read_input_tokens": 200}))
        .unwrap();
        assert_eq!(c.prompt_tokens, 300);
        assert_eq!(c.cache_write_tokens, 50);
        assert_eq!(c.cache_read_tokens, 200);
    }

    #[test]
    fn detail_cache_writes_fold_once() {
        // The KNOWN GAP shape: detail-object cache writes are inside
        // prompt_tokens and get folded out at 1.25x, not billed twice.
        let c = norm(json!({"prompt_tokens": 300, "completion_tokens": 10,
                            "prompt_tokens_details": {"cache_creation_tokens": 80}}))
        .unwrap();
        assert_eq!(c.prompt_tokens, 220);
        assert_eq!(c.cache_write_tokens, 80);
    }

    #[test]
    fn empty_usage_is_none_and_strings_do_not_count() {
        assert!(norm(json!({})).is_none());
        assert!(norm(json!({"prompt_tokens": "500"})).is_none()); // TS n(): a string is 0
        assert!(normalize_usage(None).is_none());
        // reasoning is clamped inside completion (providers fold it in).
        let c = norm(json!({"prompt_tokens": 10, "completion_tokens": 30,
                            "completion_tokens_details": {"reasoning_tokens": 99}}))
        .unwrap();
        assert_eq!(c.reasoning_tokens, 30);
    }

    #[test]
    fn estimate_is_chars_over_four_ceil() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(8), 2);
        assert_eq!(estimate_tokens(9), 3);
    }
}

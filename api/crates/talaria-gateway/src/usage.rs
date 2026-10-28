// Token ledger: usage normalization across every provider shape, the chars/4
// estimate, the priced rolling-window spend read (budget check's read side),
// and the gateway's usage_events insert.

use serde_json::Value;
use sqlx::PgPool;
use talaria_body::js_num;

/// Set from the api binary so an unpriced cloud row can nudge the oracle
/// without this crate depending on price_oracle (scheduler still lives
/// in the monolith).
pub static NUDGE_AUTO_PRICES: std::sync::OnceLock<fn(&sqlx::PgPool)> = std::sync::OnceLock::new();

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

/// Read a number: only a JSON number counts (a string digit is 0, never
/// parsed), clamped to non-negative integers.
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
/// counts. Shape detection reads the payload (not the provider name); the
/// KNOWN GAP on folded cache writes is deliberate.
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
        // Detail-object cache writes are folded in (the KNOWN GAP).
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

// The priced view. Local rows are $0. A provider-reported dollar amount on the
// row wins — that is the charge, including whichever upstream endpoint served
// an OpenRouter call. Otherwise a cloud row is derived from `provider_prices`
// (the provider's published rate, stored with source and fetched-at). Editable
// `model_prices` / `price_in_per_mtok` / `auto_prices` are not read.
//
// OpenRouter publishes a different price per endpoint and that price moves.
// A derived fallback there requires the variant that served the call. A
// model-level published price (variant '') is a fallback only for providers
// that do not route across priced endpoints.
//
// Multipliers per input KIND are VALUES interpolated into this text, not
// binds: the view is spliced into larger statements whose callers bind their
// own $1/$2.
static PRICED_STR: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "select u.*, \
        case \
          when u.endpoint_class = 'local' then 0 \
          when u.provider_cost is not null then u.provider_cost \
          when u.endpoint_class = 'cloud' \
               and pp.in_tok is not null and pp.out_tok is not null then \
            ((u.prompt_tokens + u.cache_write_tokens * {CACHE_WRITE_MULTIPLIER} + u.cache_read_tokens * {CACHE_READ_MULTIPLIER}) \
           * pp.in_tok + u.completion_tokens * pp.out_tok) / 1e6 \
          else null \
        end as cost, \
        case \
          when u.endpoint_class = 'local' then 'local' \
          when u.provider_cost is not null then 'provider' \
          when u.endpoint_class = 'cloud' \
               and pp.in_tok is not null and pp.out_tok is not null then 'derived' \
          else null \
        end as cost_basis \
      from usage_events u \
      left join llm_endpoints e on e.name = u.endpoint \
      left join lateral ( \
        select p.price_in_per_mtok as in_tok, p.price_out_per_mtok as out_tok \
        from provider_prices p \
        where p.endpoint_id = e.id \
          and p.model = u.llm_model \
          and ( \
            (coalesce(u.cost_variant, '') <> '' and p.variant = u.cost_variant) \
            or ( \
              coalesce(u.cost_variant, '') = '' \
              and p.variant = '' \
              and e.provider is distinct from 'openrouter' \
            ) \
          ) \
        order by p.fetched_at desc \
        limit 1 \
      ) pp on true"
    )
});

/// The priced view as &str, for format! interpolation.
fn priced() -> &'static str {
    &PRICED_STR
}

/// Billable spend over a rolling window, optionally for one caller
/// (`agent_model` — a fleet model id for persona rows, `api:<key>` for
/// gateway rows, `app:<slug>` for an app's, which sums every caller stamp
/// under it). Called before every budgeted call and cached by budget.rs.
pub async fn spend_since(
    pg: &PgPool,
    window_hours: i64,
    agent_model: Option<&str>,
) -> Result<SpendWindow, sqlx::Error> {
    let priced = priced();
    // An i32, not the i64 it arrives as: sqlx sends an i64 as `bigint`, and
    // `make_interval` has only an `int4` signature — the bigint bind failed
    // every read ("function make_interval(hours => bigint) does not exist"),
    // and `check_budget` fails open on a failed read, so no budget ever bound.
    let hours = window_hours.clamp(1, 24 * 365) as i32;
    // An app's subject (`app:<slug>`) is the app's whole spend: every row its
    // runs booked under their caller stamps (`app:<slug>:…`, app_keys.rs).
    let family = agent_model.and_then(crate::app_keys::app_family_prefix);
    // sqlx can't interpolate the interval literal as a bind for `now() - $1`
    // with unit attached — build `make_interval(hours => $1)` instead, which
    // stays a bound parameter.
    let sql = format!(
        "with priced as ({priced}) \
         select coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens), 0)::bigint as tokens, \
                coalesce(sum(cost), 0)::float8 as cost, \
                coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where cost is null), 0)::bigint as unpriced \
         from priced \
         where created_at > now() - make_interval(hours => $1) \
           and ($2::text is null or agent_model = $2 \
                or ($3::text is not null and left(agent_model, length($3)) = $3))"
    );
    // AssertSqlSafe: the only interpolation is the PRICED const above — no
    // caller text reaches this string, and every runtime value stays a bind.
    let row: Option<(i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(hours)
        .bind(agent_model)
        .bind(family)
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

/// Which measure of a window's spend a ceiling counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpendUnit {
    Tokens,
    Usd,
}

/// WHEN A ROLLING WINDOW'S SPEND FALLS BACK UNDER `cap`, if nothing more is
/// spent: the epoch second the oldest rows have aged out far enough. Each row
/// leaves the window `window_hours` after it was written, so walking the
/// window's rows oldest-first, the answer is the moment the first row whose
/// running total takes the rest under the cap leaves. The rows, the subject
/// and the priced figures are `spend_since`'s, so the total this walks is the
/// one the budget just refused on. `None` when the window is already under
/// the cap, or empty.
///
/// It is the honest answer to "when can this caller spend again" for a 24h
/// window — usually hours, not the minutes a fixed `Retry-After` implies.
pub async fn spend_clears_at(
    pg: &PgPool,
    window_hours: i64,
    agent_model: Option<&str>,
    unit: SpendUnit,
    cap: f64,
) -> Result<Option<i64>, sqlx::Error> {
    let priced = priced();
    let hours = window_hours.clamp(1, 24 * 365) as i32;
    let family = agent_model.and_then(crate::app_keys::app_family_prefix);
    let sql = format!(
        "with priced as ({priced}), \
         w as ( \
           select id, created_at, \
                  case when $4 then coalesce(cost, 0)::float8 \
                       else (prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens)::float8 \
                  end as amount \
           from priced \
           where created_at > now() - make_interval(hours => $1) \
             and ($2::text is null or agent_model = $2 \
                  or ($3::text is not null and left(agent_model, length($3)) = $3)) \
         ), \
         r as ( \
           select created_at, \
                  sum(amount) over (order by created_at, id rows between unbounded preceding and current row) as aged, \
                  sum(amount) over () as used \
           from w \
         ) \
         select ceil(extract(epoch from created_at + make_interval(hours => $1)))::bigint \
         from r \
         where used >= $5 and used - aged < $5 \
         order by created_at \
         limit 1"
    );
    // AssertSqlSafe: the only interpolation is the PRICED const — every
    // runtime value is a bind.
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(hours)
        .bind(agent_model)
        .bind(family)
        .bind(unit == SpendUnit::Usd)
        .bind(cap)
        .fetch_optional(pg)
        .await
}

// ── The token ledger overview (GET /api/cost) ────────────────────────────────

/// One window's aggregate, in wire order.
#[derive(Debug, serde::Serialize)]
pub struct CostWindow {
    pub prompt: i32,
    pub completion: i32,
    pub cache: i32,
    pub generations: i32,
    pub cost: serde_json::Number,
}

#[derive(Debug, serde::Serialize)]
pub struct CostSplit {
    local: i64,
    cloud: i64,
    other: i64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTotals {
    pub today: CostWindow,
    pub week: CostWindow,
    pub month: CostWindow,
    pub estimated_share: serde_json::Number,
    pub split: CostSplit,
    pub unpriced_cloud_tokens: i64,
    /// Provider activity/cost-report total for the last 30 days, when a
    /// provider returned one. The month tile uses this number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_reported: Option<ProviderReported>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReported {
    pub cost: serde_json::Number,
    pub source: String,
    pub fetched_at: Option<String>,
    /// The freshest activity row is older than the freshness window — the
    /// month tile shows the ledger number, not this stale report.
    pub stale: bool,
}

/// Provider activity counts as fresh for 12h — twice the 6h price-refresh
/// cadence, so a working ingest never reads as stale.
pub const PROVIDER_REPORTED_FRESH_SECS: f64 = 12.0 * 3600.0;

/// Pure: is the freshest activity row (age in seconds, straight from the
/// query) older than the freshness window? No age reads as stale — never
/// trust what we can't date.
fn provider_reported_stale(age_secs: Option<f64>) -> bool {
    age_secs.is_none_or(|a| a > PROVIDER_REPORTED_FRESH_SECS)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPerModel {
    llm_model: Option<String>,
    endpoint_class: Option<String>,
    tokens: i64,
    cost: Option<serde_json::Number>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPerAgent {
    agent_model: Option<String>,
    prompt: i32,
    completion: i32,
    generations: i32,
    last_used: Option<String>,
    cost: serde_json::Number,
    local_share: Option<serde_json::Number>,
}

#[derive(Debug, serde::Serialize)]
pub struct CostPerDay {
    day: String,
    prompt: i32,
    completion: i32,
    generations: i32,
    local: i32,
    cloud: i32,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostOverview {
    pub totals: CostTotals,
    pub per_model: Vec<CostPerModel>,
    pub per_agent: Vec<CostPerAgent>,
    pub per_day: Vec<CostPerDay>,
}

/// The token ledger overview: totals, per-model,
/// per-agent, per-day — the Observability cost page's whole read. The nine
/// aggregates are independent and run concurrently
/// (alerts + /cost call it on every load).
pub async fn cost_overview(pg: &PgPool) -> Result<CostOverview, sqlx::Error> {
    let priced = priced();
    // AssertSqlSafe everywhere below: each string interpolates only the
    // PRICED const — intervals go in as make_interval binds, never text.
    let window = |days: i32| {
        let sql = format!(
            "with priced as ({priced}) \
             select coalesce(sum(prompt_tokens),0)::int as prompt, \
                    coalesce(sum(completion_tokens),0)::int as completion, \
                    coalesce(sum(cache_write_tokens + cache_read_tokens),0)::int as cache, \
                    count(*)::int as generations, \
                    coalesce(sum(cost), 0)::float8 as cost \
             from priced where created_at > now() - make_interval(days => $1)"
        );
        let q = sqlx::query_as::<_, (i32, i32, i32, i32, f64)>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(days);
        async move { q.fetch_one(pg).await }
    };
    let est = async {
        let row: (i32, i32) = sqlx::query_as(
            "select count(*) filter (where estimated)::int as est, count(*)::int as all_n \
             from usage_events where created_at > now() - make_interval(days => $1)",
        )
        .bind(30)
        .fetch_one(pg)
        .await?;
        Ok::<_, sqlx::Error>(row)
    };
    let split = async {
        sqlx::query_as::<_, (i64, i64, i64)>(
            "select coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'local'), 0)::bigint as local, \
                   coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'cloud'), 0)::bigint as cloud, \
                   coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class is null), 0)::bigint as other \
             from usage_events where created_at > now() - make_interval(days => $1)",
        )
        .bind(30)
        .fetch_one(pg)
        .await
    };
    let unpriced = async {
        let sql = format!(
            "with priced as ({priced}) \
             select coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens), 0)::bigint as tokens \
             from priced where created_at > now() - make_interval(days => $1) \
               and endpoint_class = 'cloud' and cost is null"
        );
        let row: (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(30)
            .fetch_one(pg)
            .await?;
        Ok::<_, sqlx::Error>(row)
    };
    // (llmModel, endpointClass, tokens, cost)
    type PerModelRow = (Option<String>, Option<String>, i64, Option<f64>);
    // (agentModel, prompt, completion, generations, last_ms, cost, localShare)
    type PerAgentRow = (Option<String>, i32, i32, i32, Option<i64>, f64, Option<f64>);

    let per_model = async {
        let sql = format!(
            "with priced as ({priced}) \
             select llm_model as \"llmModel\", endpoint_class as \"endpointClass\", \
                    coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens), 0)::bigint as tokens, \
                    sum(cost)::float8 as cost \
             from priced where created_at > now() - make_interval(days => $1) \
             group by llm_model, endpoint_class \
             order by (endpoint_class = 'local') desc nulls last, tokens desc"
        );
        let rows: Vec<PerModelRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(30)
            .fetch_all(pg)
            .await?;
        Ok::<_, sqlx::Error>(
            rows.into_iter()
                .map(|(llm_model, endpoint_class, tokens, cost)| CostPerModel {
                    llm_model,
                    endpoint_class,
                    tokens,
                    cost: cost.map(js_num),
                })
                .collect(),
        )
    };
    let per_agent = async {
        let sql = format!(
            "with priced as ({priced}) \
             select agent_model as \"agentModel\", \
                    coalesce(sum(prompt_tokens),0)::int as prompt, \
                    coalesce(sum(completion_tokens),0)::int as completion, \
                    count(*)::int as generations, \
                    (trunc(extract(epoch from max(created_at)) * 1000))::bigint as last_ms, \
                    coalesce(sum(cost), 0)::float8 as cost, \
                    case when count(*) filter (where endpoint_class is not null) = 0 then null \
                         else sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'local')::float8 \
                              / nullif(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class is not null), 0) \
                    end as \"localShare\" \
             from priced where created_at > now() - make_interval(days => $1) \
             group by agent_model \
             order by sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) desc"
        );
        let rows: Vec<PerAgentRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(30)
            .fetch_all(pg)
            .await?;
        Ok::<_, sqlx::Error>(
            rows.into_iter()
                .map(
                    |(agent_model, prompt, completion, generations, last_ms, cost, local_share)| {
                        CostPerAgent {
                            agent_model,
                            prompt,
                            completion,
                            generations,
                            last_used: last_ms.map(talaria_agent_auth::epoch_ms_to_iso),
                            cost: js_num(cost),
                            local_share: local_share.map(js_num),
                        }
                    },
                )
                .collect(),
        )
    };
    let per_day = async {
        let rows: Vec<(String, i32, i32, i32, i32, i32)> = sqlx::query_as(
            "select to_char(date_trunc('day', created_at), 'YYYY-MM-DD') as day, \
                    coalesce(sum(prompt_tokens),0)::int as prompt, \
                    coalesce(sum(completion_tokens),0)::int as completion, \
                    count(*)::int as generations, \
                    coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'local'), 0)::int as local, \
                    coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'cloud'), 0)::int as cloud \
             from usage_events where created_at > now() - make_interval(days => $1) \
             group by 1 order by 1 asc",
        )
        .bind(14)
        .fetch_all(pg)
        .await?;
        Ok::<_, sqlx::Error>(
            rows.into_iter()
                .map(
                    |(day, prompt, completion, generations, local, cloud)| CostPerDay {
                        day,
                        prompt,
                        completion,
                        generations,
                        local,
                        cloud,
                    },
                )
                .collect(),
        )
    };

    let (today, week, month, est, split, unpriced, per_model, per_agent, per_day) = tokio::join!(
        window(1),
        window(7),
        window(30),
        est,
        split,
        unpriced,
        per_model,
        per_agent,
        per_day
    );
    let (today, week, month) = (today?, week?, month?);
    let (est, split, (unpriced,), per_model, per_agent, per_day) =
        (est?, split?, unpriced?, per_model?, per_agent?, per_day?);
    let activity: (i32, f64, Option<String>, Option<String>, Option<f64>) = sqlx::query_as(
        "select count(*)::int, coalesce(sum(cost_usd), 0)::float8, max(source), \
         to_char(max(fetched_at) at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), \
         extract(epoch from (now() - max(fetched_at)))::float8 \
         from provider_spend \
         where source in ('openrouter.activity', 'openai.costs', 'anthropic.cost_report') \
           and window_start > now() - make_interval(days => $1)",
    )
    .bind(30)
    .fetch_one(pg)
    .await?;
    let provider_reported = (activity.0 > 0).then(|| ProviderReported {
        cost: js_num(activity.1),
        source: activity.2.unwrap_or_else(|| "provider".to_string()),
        fetched_at: activity.3,
        stale: provider_reported_stale(activity.4),
    });
    let w =
        |(prompt, completion, cache, generations, cost): (i32, i32, i32, i32, f64)| CostWindow {
            prompt,
            completion,
            cache,
            generations,
            cost: js_num(cost),
        };
    let mut month = w(month);
    if let Some(reported) = &provider_reported {
        // A stale report would replace the real ledger number forever —
        // only a fresh one wins the month tile.
        if !reported.stale {
            month.cost = reported.cost.clone();
        }
    }
    Ok(CostOverview {
        totals: CostTotals {
            today: w(today),
            week: w(week),
            month,
            estimated_share: js_num(if est.1 != 0 {
                f64::from(est.0) / f64::from(est.1)
            } else {
                0.0
            }),
            split: CostSplit {
                local: split.0,
                cloud: split.1,
                other: split.2,
            },
            unpriced_cloud_tokens: unpriced,
            provider_reported,
        },
        per_model,
        per_agent,
        per_day,
    })
}

// ── Ticket usage (the tasks family's detail read) ────────────────────────────

/// One per-model line of a ticket's usage.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsagePerModel {
    pub llm_model: Option<String>,
    pub tokens: i64,
    /// Null when every row for the model is unpriced — shown, never silent $0.
    pub cost: Option<serde_json::Number>,
}

/// Token spend attributed to one ticket: the metered rows that carry its id.
/// Rows agents once self-reported through the retired MCP `log_usage` tool
/// (`source = 'ticket'`) still count here.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub cache_write_tokens: i32,
    pub cache_read_tokens: i32,
    pub cost: serde_json::Number,
    /// Tokens with no $ figure: unpriced cloud AND unattributed rows (agent
    /// name didn't resolve to a def). Cost is understated when > 0.
    pub unpriced_tokens: i64,
    pub per_model: Vec<TaskUsagePerModel>,
}

pub async fn task_usage(pg: &PgPool, task_id: &str) -> Result<TaskUsage, sqlx::Error> {
    let priced = priced();
    // AssertSqlSafe: the only interpolation is the PRICED const — the task id
    // stays a bind.
    let totals_sql = format!(
        "with priced as ({priced}) \
         select coalesce(sum(prompt_tokens),0)::int as prompt, \
                coalesce(sum(completion_tokens),0)::int as completion, \
                coalesce(sum(cache_write_tokens),0)::int as cache_write, \
                coalesce(sum(cache_read_tokens),0)::int as cache_read, \
                coalesce(sum(cost),0)::float8 as cost, \
                coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + \
                             cache_read_tokens) filter (where cost is null), 0)::bigint as unpriced \
         from priced where task_id = $1::uuid"
    );
    let (prompt, completion, cache_write, cache_read, cost, unpriced): (
        i32,
        i32,
        i32,
        i32,
        f64,
        i64,
    ) = sqlx::query_as(sqlx::AssertSqlSafe(totals_sql.as_str()))
        .bind(task_id)
        .fetch_one(pg)
        .await?;
    let per_model_sql = format!(
        "with priced as ({priced}) \
         select llm_model as \"llmModel\", \
                coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + \
                             cache_read_tokens), 0)::bigint as tokens, \
                sum(cost)::float8 as cost \
         from priced where task_id = $1::uuid \
         group by llm_model order by tokens desc"
    );
    let rows: Vec<(Option<String>, i64, Option<f64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(per_model_sql.as_str()))
            .bind(task_id)
            .fetch_all(pg)
            .await?;
    Ok(TaskUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        cache_write_tokens: cache_write,
        cache_read_tokens: cache_read,
        cost: js_num(cost),
        unpriced_tokens: unpriced,
        per_model: rows
            .into_iter()
            .map(|(llm_model, tokens, cost)| TaskUsagePerModel {
                llm_model,
                tokens,
                cost: cost.map(js_num),
            })
            .collect(),
    })
}

/// Ledger row for a gateway call. Attribution is
/// direct (we KNOW the endpoint), no agent-def classification involved.
/// `k` clamps negatives to 0.
fn k(v: i64) -> i64 {
    v.max(0)
}

/// Dollars the provider reported for one call, captured off the completion
/// body. Absent when the provider sent no cost — the row stays unpriced until
/// a generation lookup or a published-price derivation fills it.
#[derive(Debug, Clone, Default)]
pub struct ReportedSpend {
    pub cost_usd: Option<f64>,
    pub provider: Option<String>,
    pub variant: Option<String>,
    pub generation_id: Option<String>,
}

/// Pull a provider-reported cost and generation id out of a completion body.
/// `usage.cost` is the charge (OpenRouter always sends it). A `gen-` id is
/// OpenRouter's generation id, kept so a later lookup can fill a missing cost
/// and the upstream endpoint that served the call.
pub fn reported_spend(provider: &str, body: &Value) -> ReportedSpend {
    let usage = body.get("usage").filter(|u| !u.is_null());
    let cost_usd = usage
        .and_then(|u| json_f64(u.get("cost")))
        .filter(|n| n.is_finite() && *n >= 0.0);
    let generation_id = body
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("gen-") || provider == "openrouter")
        .map(str::to_string);
    let variant = body
        .get("provider")
        .and_then(Value::as_str)
        .or_else(|| body.get("provider_name").and_then(Value::as_str))
        .map(str::to_string);
    ReportedSpend {
        cost_usd,
        provider: (!provider.is_empty()).then(|| provider.to_string()),
        variant,
        generation_id,
    }
}

fn json_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Whether a published price row may price this call. A known serving variant
/// matches only that row. With no variant, a model-level published price is a
/// derived fallback — except OpenRouter, whose catalog price is not the
/// endpoint that served the call.
pub fn published_price_applies(
    provider: &str,
    served_variant: Option<&str>,
    price_variant: &str,
) -> bool {
    if let Some(served) = served_variant.filter(|v| !v.is_empty()) {
        return price_variant == served;
    }
    price_variant.is_empty() && provider != "openrouter"
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
    record_gateway_usage_with_spend(
        pg,
        caller,
        endpoint_name,
        endpoint_class,
        upstream_model,
        counts,
        estimated,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn record_gateway_usage_with_spend(
    pg: &PgPool,
    caller: &str,
    endpoint_name: &str,
    endpoint_class: &str,
    upstream_model: &str,
    counts: &TokenCounts,
    estimated: bool,
    spend: Option<&ReportedSpend>,
) -> Result<(), sqlx::Error> {
    record_relay_usage(
        pg,
        RELAY_SOURCE,
        caller,
        None,
        &StatedFlow::default(),
        endpoint_name,
        endpoint_class,
        upstream_model,
        counts,
        estimated,
        spend,
    )
    .await
}

/// The flow a caller states for its own rows, beside the one a recorded
/// affinity key would give them: an app run's ref and ticket. Each is booked
/// only when it is a uuid — a ref the app made up (a support message id, a
/// slug) is dropped from the row, never the row with it. A flow recorded for
/// the call's affinity key still wins.
#[derive(Debug, Clone, Default)]
pub struct StatedFlow {
    pub ref_id: Option<String>,
    pub task_id: Option<String>,
}

/// The source a relay row carries when the calling key is the writer of
/// record — an operator's own key, a harness run on the workbench key.
pub const RELAY_SOURCE: &str = "gateway";
/// The source a relay row carries for an agent's call that no Talaria flow
/// claimed: a Hermes cron (its affinity key is `cron_<job id>`), a call made
/// outside any conversation (no key at all), or a turn whose key the
/// transport never recorded. A call under a key the transport DID record
/// carries that flow's source instead — see "Attribution" below.
pub const UNATTENDED_SOURCE: &str = "unattended";

/// One relay-metered row under `source`. `record_gateway_usage_with_spend`
/// is this with the operator-key source and no affinity key; the relay picks
/// the source by who the caller is.
///
/// `affinity_key` is the `x-talaria-session` value Hermes stamped on an
/// agent's call. When the chat transport recorded which flow that key — or
/// the conversation the agent's plugin tied it to — belongs to, the row
/// takes the flow's source, `ref_id` and `task_id` in the same statement;
/// otherwise it keeps `source`, and a later record may backfill it.
#[allow(clippy::too_many_arguments)]
pub async fn record_relay_usage(
    pg: &PgPool,
    source: &str,
    caller: &str,
    affinity_key: Option<&str>,
    stated: &StatedFlow,
    endpoint_name: &str,
    endpoint_class: &str,
    upstream_model: &str,
    counts: &TokenCounts,
    estimated: bool,
    spend: Option<&ReportedSpend>,
) -> Result<(), sqlx::Error> {
    let spend = spend.cloned().unwrap_or_default();
    let cost_source = spend.cost_usd.map(|_| "provider");
    let reported_cost = spend.cost_usd.is_some();
    sqlx::query(
        "insert into usage_events (agent_model, source, ref_id, task_id, affinity_key, \
                                  prompt_tokens, completion_tokens, \
                                  cache_write_tokens, cache_read_tokens, reasoning_tokens, \
                                  estimated, endpoint_class, llm_model, endpoint, \
                                  provider_cost, cost_source, cost_provider, cost_variant, \
                                  cost_fetched_at, generation_id) \
         select $1, coalesce(a.source, $16), coalesce(a.ref_id, $18::uuid), \
                coalesce(a.task_id, $19::uuid), $17, \
                $2, $3, $4, $5, $6, $7, $8, $9, $10, \
                $11, $12, $13, $14, case when $11::numeric is null then null else now() end, $15 \
         from (values (1)) as one (x) \
         left join lateral ( \
           select a.source, a.ref_id, a.task_id from usage_attributions a \
            where a.agent_model = $1 \
              and a.affinity_key in ( \
                    $17, \
                    (select k.conversation_key from usage_affinity_keys k \
                      where k.agent_model = $1 and k.affinity_key = $17)) \
            order by (a.affinity_key = $17) desc \
            limit 1 \
         ) a on true",
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
    .bind(spend.cost_usd)
    .bind(cost_source)
    .bind(spend.provider)
    .bind(spend.variant.filter(|v| !v.is_empty()))
    .bind(spend.generation_id)
    .bind(source)
    .bind(affinity_key)
    .bind(uuid_or_none(stated.ref_id.as_deref()))
    .bind(uuid_or_none(stated.task_id.as_deref()))
    .execute(pg)
    .await?;
    // A cloud row with no charge of its own is priced from a published rate;
    // when there is none, the oracle is told — as the per-turn ledger rows
    // this writer replaced for agents used to tell it.
    if endpoint_class == "cloud" && !reported_cost {
        nudge_if_unpriced(pg, endpoint_name, upstream_model);
    }
    Ok(())
}

/// Nudge the price oracle when `model` on `endpoint` has no published price
/// to derive a cloud row's cost from — detached, so no write ever waits on
/// it. The relay's ledger writer (`record_relay_usage`, which every booked
/// call goes through) calls it for a cloud row that carries no cost of its
/// own.
fn nudge_if_unpriced(pg: &PgPool, endpoint: &str, model: &str) {
    if endpoint.is_empty() || model.is_empty() {
        return;
    }
    let (pg, endpoint, model) = (pg.clone(), endpoint.to_string(), model.to_string());
    tokio::spawn(async move {
        let priced = sqlx::query(
            "select 1 as ok from provider_prices p \
             join llm_endpoints e on e.id = p.endpoint_id \
             where e.name = $1 and p.model = $2 \
               and (p.variant <> '' or e.provider is distinct from 'openrouter')",
        )
        .bind(&endpoint)
        .bind(&model)
        .fetch_optional(&pg)
        .await;
        if matches!(priced, Ok(None))
            && let Some(nudge) = NUDGE_AUTO_PRICES.get()
        {
            nudge(&pg);
        }
    });
}

// ── Attribution: which flow an agent's call belongs to ───────────────────────
//
// EVERY CALL ON AN AGENT'S KEY IS BOOKED AT THE RELAY, one row per upstream
// call, under the agent. Nothing else writes an agent's spend: the flow-level
// rows a chat, channel or harness turn used to write (one per turn, from the
// counters Hermes reports for its main loop) never saw the auxiliary calls
// behind a turn — compression, titles, vision, web_extract, session_search —
// nor a delegated subagent's, and the turn register that made the relay skip
// every call while a turn was open also skipped any cron that fired inside
// one. Booking every call where it is made is what closes all of those, and
// it lets a budget see a long turn's spend while the turn is still running.
//
// WHOSE TURN IT IS rides on the call itself. The render gives every agent a
// `providers.talaria` entry on the gateway URL its model specs already use,
// with `session_affinity_header: x-talaria-session` (fleet-brain's
// `gateway_provider_entry`); Hermes matches a provider entry to a call by
// base URL and stamps that header on every request to it — the main loop on
// any transport, every auxiliary call, a subagent (its parent's key) and a
// cron fire. The value is Hermes' affinity key for the conversation:
//
//   a cron fire          `cron_<job id>`, every fire of one job alike
//   an API turn          `gwk_<digest>` — on v0.22 every turn Talaria sends
//                        runs under Hermes' session authority, which names
//                        the session's route, and the affinity key is then
//                        a digest of (source, route, generation): stable for
//                        the conversation, and computable only inside Hermes
//   a turn with no route the conversation's own id (older Hermes)
//   anything else        no header at all
//
// So the key on the wire is not a name Talaria knows, and two records meet
// in the middle:
//
//   usage_attributions   written by the chat transport (fleet_chat.rs): the
//                        conversation it drove — the session id it created,
//                        or on the completions arm the id Hermes answers in
//                        `X-Hermes-Session-Id` — with the flow's source,
//                        ref_id and task_id
//   usage_affinity_keys  written by the agent's talaria-events plugin from
//                        inside Hermes (POST /api/agents/affinity): "the key
//                        I stamp is conversation C's", read off the same
//                        function that builds the header, at the start of
//                        every turn
//
// The relay copies the flow onto each row it books — under the conversation's
// own id, or under a key the plugin tied to it — in the insert itself. Either
// record can land second (the plugin reports when the turn starts; the
// completions arm learns its id from the response headers), so whichever
// lands second BACKFILLS the rows already booked, and the transport backfills
// once more when the stream is let go.
//
// WHAT A KEY CAN AND CANNOT DO: it attributes within one agent and nothing
// more. Both records are keyed by agent, the relay's caller is the agent its
// bearer names, and the plugin's door takes only the agent's own credential —
// so a forged header or report (an agent with a terminal can send anything)
// can only move spend between that agent's own flows; the budget subject is
// still the key. Two completions conversations that open with the same system
// prompt and first message share Hermes' session (its id is a digest of
// exactly those), and the later turn's attribution wins.

/// The request header Hermes stamps the conversation key on — the name the
/// render writes as `providers.talaria.session_affinity_header`.
pub const SESSION_HEADER: &str = "x-talaria-session";

/// The longest affinity key the relay keeps. Hermes' own are short (a uuid,
/// `api-` plus 16 hex digits, `cron_` plus a job id, `gwk_` plus 24); this is
/// a bound on what a forged header can write, not a format.
const AFFINITY_KEY_MAX: usize = 128;

/// An affinity key as the relay and the transport keep it: trimmed, 1 to
/// `AFFINITY_KEY_MAX` printable ASCII characters with no spaces. Anything else
/// is no key — the call is still booked, just unattributed.
pub fn affinity_key(raw: Option<&str>) -> Option<String> {
    let key = raw?.trim();
    (!key.is_empty() && key.len() <= AFFINITY_KEY_MAX && key.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| key.to_string())
}

/// The flow a Talaria-driven turn belongs to, handed to the chat transport so
/// every call Hermes makes for the turn is booked under it. `source` is the
/// ledger's word for the flow (`chat`, `channel`, `ticket`, `research`);
/// `ref_id` the conversation, channel or run, and `task_id` the ticket —
/// both uuids, and anything that is not one is dropped rather than failing
/// the record.
#[derive(Debug, Clone)]
pub struct TurnAttribution {
    pub pg: PgPool,
    /// The agent as the relay names it — its model id, never a tier's.
    pub agent_model: String,
    pub source: String,
    pub ref_id: Option<String>,
    pub task_id: Option<String>,
}

fn uuid_or_none(v: Option<&str>) -> Option<String> {
    v.filter(|s| uuid::Uuid::parse_str(s).is_ok())
        .map(str::to_string)
}

/// Record that `key` is this turn's: rows the relay books under it from now
/// on carry the turn's source, ref_id and task_id. Idempotent — the same key
/// on a later turn re-records it, and the latest flow wins.
pub async fn record_attribution(a: &TurnAttribution, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into usage_attributions (agent_model, affinity_key, source, ref_id, task_id) \
         values ($1, $2, $3, $4::uuid, $5::uuid) \
         on conflict (agent_model, affinity_key) do update \
           set source = excluded.source, ref_id = excluded.ref_id, \
               task_id = excluded.task_id, updated_at = now()",
    )
    .bind(&a.agent_model)
    .bind(key)
    .bind(&a.source)
    .bind(uuid_or_none(a.ref_id.as_deref()))
    .bind(uuid_or_none(a.task_id.as_deref()))
    .execute(&a.pg)
    .await
    .map(|_| ())
}

/// Claim the rows the relay booked for conversation `key` before both of its
/// records were in — under the conversation's own id, or under an affinity
/// key the agent's plugin tied to it. Only `unattended` rows of the last day
/// move: a row already attributed is left alone, and a key that comes round
/// again much later (a reused completions digest) cannot reach back over
/// another flow's history. Answers how many rows it claimed; a conversation
/// with no flow recorded yet claims none.
pub async fn backfill_attribution(
    pg: &PgPool,
    agent_model: &str,
    key: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "update usage_events u \
            set source = a.source, ref_id = a.ref_id, task_id = a.task_id \
           from usage_attributions a \
          where a.agent_model = $1 and a.affinity_key = $2 \
            and u.agent_model = $1 \
            and (u.affinity_key = $2 \
                 or u.affinity_key in (select k.affinity_key from usage_affinity_keys k \
                                        where k.agent_model = $1 and k.conversation_key = $2)) \
            and u.source = $3 \
            and u.created_at > now() - interval '1 day'",
    )
    .bind(agent_model)
    .bind(key)
    .bind(UNATTENDED_SOURCE)
    .execute(pg)
    .await
    .map(|r| r.rows_affected())
}

/// Record the agent's plugin's report that the affinity key it stamps on its
/// calls belongs to `conversation` — the conversation's own id as Talaria
/// knows it — then claim what was already booked under it. The latest report
/// for a key wins. Answers how many rows the backfill claimed.
pub async fn record_affinity_key(
    pg: &PgPool,
    agent_model: &str,
    key: &str,
    conversation: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "insert into usage_affinity_keys (agent_model, affinity_key, conversation_key) \
         values ($1, $2, $3) \
         on conflict (agent_model, affinity_key) do update \
           set conversation_key = excluded.conversation_key, updated_at = now()",
    )
    .bind(agent_model)
    .bind(key)
    .bind(conversation)
    .execute(pg)
    .await?;
    backfill_attribution(pg, agent_model, conversation).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_numbers_print_like_json_stringify() {
        // Integral floats lose the ".0" — zero prints "0".
        assert_eq!(js_num(0.0).to_string(), "0");
        assert_eq!(js_num(1.0).to_string(), "1");
        assert_eq!(js_num(-2.0).to_string(), "-2");
        // Fractional values keep their shortest round-trip form.
        assert_eq!(js_num(0.113816095).to_string(), "0.113816095");
        assert_eq!(
            js_num(0.00688802543270929).to_string(),
            "0.00688802543270929"
        );
    }

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
        assert!(norm(json!({"prompt_tokens": "500"})).is_none()); // a string is 0, never parsed
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
    #[test]
    fn reported_spend_reads_the_provider_charge_not_a_local_rate() {
        let body = json!({
            "id": "gen-abc",
            "provider": "Anthropic",
            "usage": { "prompt_tokens": 10, "completion_tokens": 2, "cost": 0.0015 }
        });
        let spend = reported_spend("openrouter", &body);
        assert_eq!(spend.cost_usd, Some(0.0015));
        assert_eq!(spend.generation_id.as_deref(), Some("gen-abc"));
        assert_eq!(spend.variant.as_deref(), Some("Anthropic"));
    }

    #[test]
    fn provider_reported_is_stale_past_the_freshness_window_or_without_an_age() {
        // Fresh: six minutes old.
        assert!(!provider_reported_stale(Some(360.0)));
        // Exactly 12h old is still fresh; a second past is stale.
        assert!(!provider_reported_stale(Some(12.0 * 3600.0)));
        assert!(provider_reported_stale(Some(12.0 * 3600.0 + 1.0)));
        // Clearly old: a week.
        assert!(provider_reported_stale(Some(7.0 * 86_400.0)));
        // No age at all: stale, never trusted.
        assert!(provider_reported_stale(None));
    }

    #[test]
    fn openrouter_without_a_serving_variant_does_not_take_a_model_level_price() {
        assert!(!published_price_applies("openrouter", None, ""));
        assert!(published_price_applies(
            "openrouter",
            Some("Infermatic"),
            "Infermatic"
        ));
        assert!(!published_price_applies(
            "openrouter",
            Some("Infermatic"),
            "OpenAI"
        ));
        assert!(published_price_applies("openai", None, ""));
    }

    #[test]
    fn an_affinity_key_is_short_printable_ascii_or_nothing() {
        assert_eq!(
            affinity_key(Some(" 4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a ")).as_deref(),
            Some("4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a"),
            "trimmed"
        );
        assert_eq!(
            affinity_key(Some("api-0123456789abcdef")).as_deref(),
            Some("api-0123456789abcdef")
        );
        assert_eq!(
            affinity_key(Some("cron_a1b2c3")).as_deref(),
            Some("cron_a1b2c3")
        );
        assert_eq!(affinity_key(None), None);
        assert_eq!(affinity_key(Some("   ")), None);
        assert_eq!(affinity_key(Some("two words")), None, "no spaces");
        assert_eq!(affinity_key(Some("caf\u{e9}")), None, "ASCII only");
        assert_eq!(affinity_key(Some("a\nb")), None, "no control characters");
        assert_eq!(
            affinity_key(Some(&"k".repeat(128))).map(|k| k.len()),
            Some(128)
        );
        assert_eq!(affinity_key(Some(&"k".repeat(129))), None, "bounded");
    }

    #[test]
    fn only_a_uuid_reaches_a_uuid_column() {
        assert_eq!(
            uuid_or_none(Some("4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a")).as_deref(),
            Some("4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a")
        );
        assert_eq!(uuid_or_none(Some("not-a-uuid")), None);
        assert_eq!(uuid_or_none(None), None);
    }
}

// ── Live-DB proof of the relay ledger's SQL ──────────────────────────────────
//
// Three hand-written statements decide where an agent's spend lands and
// whether a budget can see it, and one of them already failed every call
// in silence: `spend_since` bound its window as a bigint `make_interval`
// has no signature for, and `check_budget` fails open on a failed read, so
// no budget ever bound. A unit test cannot see that — it is Postgres's
// answer — so these run against a migrated database, `#[ignore]`d:
//
//   DATABASE_URL=postgres://… cargo test -p talaria-gateway --lib -- --ignored usage::live_db
//
// CI's live-tests job runs them (`cargo test --workspace --lib -- --ignored`)
// on its freshly migrated scratch database. Every test owns its own agent
// name and deletes only its own rows.

#[cfg(test)]
mod live_db {
    use super::*;

    async fn pg() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        PgPool::connect(&url).await.expect("connect")
    }

    /// A name no other test, and no real agent, books under.
    fn agent() -> String {
        format!("live-meter-{}", uuid::Uuid::new_v4())
    }

    fn uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    async fn cleanup(pg: &PgPool, agent: &str) {
        for sql in [
            "delete from usage_events where agent_model = $1",
            "delete from usage_attributions where agent_model = $1",
            "delete from usage_affinity_keys where agent_model = $1",
        ] {
            sqlx::query(sql).bind(agent).execute(pg).await.unwrap();
        }
    }

    /// One relay row, as the relay books an agent's call. `prompt` doubles
    /// as the row's name in the assertions. A local endpoint, so the row is
    /// $0 and never nudges the price oracle.
    async fn book(pg: &PgPool, source: &str, agent: &str, key: Option<&str>, prompt: i64) {
        let counts = TokenCounts {
            prompt_tokens: prompt,
            completion_tokens: 1,
            ..TokenCounts::default()
        };
        record_relay_usage(
            pg,
            source,
            agent,
            key,
            &StatedFlow::default(),
            "live-test-endpoint",
            "local",
            "live-test-model",
            &counts,
            false,
            None,
        )
        .await
        .expect("the relay's insert");
    }

    /// (source, ref_id, task_id) of the row named `prompt`.
    async fn row(
        pg: &PgPool,
        agent: &str,
        prompt: i64,
    ) -> (String, Option<String>, Option<String>) {
        sqlx::query_as(
            "select source, ref_id::text, task_id::text from usage_events \
              where agent_model = $1 and prompt_tokens = $2",
        )
        .bind(agent)
        .bind(prompt)
        .fetch_one(pg)
        .await
        .expect("the row")
    }

    fn flow(
        pg: &PgPool,
        agent: &str,
        source: &str,
        ref_id: &str,
        task_id: Option<&str>,
    ) -> TurnAttribution {
        TurnAttribution {
            pg: pg.clone(),
            agent_model: agent.to_string(),
            source: source.to_string(),
            ref_id: Some(ref_id.to_string()),
            task_id: task_id.map(str::to_string),
        }
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn spend_since_reads_its_window_instead_of_failing_open() {
        let pg = pg().await;
        let a = agent();
        cleanup(&pg, &a).await;
        book(&pg, UNATTENDED_SOURCE, &a, None, 100).await;
        book(&pg, UNATTENDED_SOURCE, &a, None, 50).await;
        sqlx::query(
            "update usage_events set created_at = now() - interval '2 days' \
              where agent_model = $1 and prompt_tokens = 50",
        )
        .bind(&a)
        .execute(&pg)
        .await
        .unwrap();

        // Each read is the statement the budget check runs; an error here is
        // a budget that never binds.
        let day = spend_since(&pg, 24, Some(&a)).await.expect("a 24h window");
        assert_eq!(day.tokens, 101, "the row two days old is outside 24h");
        assert_eq!(day.cost, 0.0, "a local row is $0");
        let three = spend_since(&pg, 72, Some(&a)).await.expect("a 72h window");
        assert_eq!(three.tokens, 152);
        // The clamp's two ends still bind as the int4 make_interval takes.
        spend_since(&pg, 0, Some(&a)).await.expect("the 1h floor");
        let wide = spend_since(&pg, i64::MAX, Some(&a))
            .await
            .expect("the one-year ceiling");
        assert_eq!(wide.tokens, 152);
        // The org-wide read (no subject) is the same statement.
        let all = spend_since(&pg, 24, None)
            .await
            .expect("the org-wide window");
        assert!(all.tokens >= 101);
        cleanup(&pg, &a).await;
    }

    /// The resume time a budget refusal names: the moment enough of the
    /// window's oldest spend has aged out to take the rest under the cap.
    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn spend_clears_at_is_when_the_oldest_rows_age_out_far_enough() {
        let pg = pg().await;
        let a = agent();
        cleanup(&pg, &a).await;
        // 401, 301 and 201 tokens (each row is prompt + 1), written 10h, 5h
        // and 1h ago: 903 in the 24h window.
        for (prompt, hours_ago) in [(400, 10), (300, 5), (200, 1)] {
            book(&pg, UNATTENDED_SOURCE, &a, None, prompt).await;
            sqlx::query(
                "update usage_events set created_at = now() - make_interval(hours => $3) \
                  where agent_model = $1 and prompt_tokens = $2",
            )
            .bind(&a)
            .bind(prompt as i32)
            .bind(hours_ago)
            .execute(&pg)
            .await
            .unwrap();
        }
        let leaves = |prompt: i32| {
            let (pg, a) = (pg.clone(), a.clone());
            async move {
                sqlx::query_scalar::<_, i64>(
                    "select ceil(extract(epoch from created_at + interval '24 hours'))::bigint \
                       from usage_events where agent_model = $1 and prompt_tokens = $2",
                )
                .bind(&a)
                .bind(prompt)
                .fetch_one(&pg)
                .await
                .unwrap()
            }
        };
        let at = |cap: f64| spend_clears_at(&pg, 24, Some(&a), SpendUnit::Tokens, cap);

        // Cap 600: the oldest row leaving takes 903 to 502.
        assert_eq!(at(600.0).await.unwrap(), Some(leaves(400).await));
        // Cap 300: two rows have to leave (903 → 502 → 201).
        assert_eq!(at(300.0).await.unwrap(), Some(leaves(300).await));
        // Cap 201: 201 left is still AT the cap — the newest row has to go.
        assert_eq!(at(201.0).await.unwrap(), Some(leaves(200).await));
        // Under the cap there is nothing to wait for.
        assert_eq!(at(1000.0).await.unwrap(), None);
        // Dollars: these rows are local, $0, so no dollar cap is reached.
        let usd = spend_clears_at(&pg, 24, Some(&a), SpendUnit::Usd, 0.01).await;
        assert_eq!(usd.unwrap(), None);
        // The org-wide read is the same statement, with no subject.
        spend_clears_at(&pg, 24, None, SpendUnit::Tokens, 1.0)
            .await
            .expect("the org-wide read");
        cleanup(&pg, &a).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_relay_row_takes_the_flow_recorded_for_its_key_in_the_insert() {
        let pg = pg().await;
        let (a, other) = (agent(), agent());
        cleanup(&pg, &a).await;
        cleanup(&pg, &other).await;
        let (conversation, ticket) = (uuid(), uuid());

        // The session arm: the key on the wire is the conversation's own id.
        record_attribution(
            &flow(&pg, &a, "ticket", &conversation, Some(&ticket)),
            &conversation,
        )
        .await
        .unwrap();
        book(&pg, UNATTENDED_SOURCE, &a, Some(&conversation), 1).await;
        assert_eq!(
            row(&pg, &a, 1).await,
            (
                "ticket".into(),
                Some(conversation.clone()),
                Some(ticket.clone())
            )
        );

        // A v0.22 turn: the key is Hermes' gwk_ digest, which the plugin
        // tied to the conversation. Nothing was booked under it yet.
        let gwk = format!("gwk_{}", &uuid()[..8]);
        assert_eq!(
            record_affinity_key(&pg, &a, &gwk, &conversation)
                .await
                .unwrap(),
            0
        );
        book(&pg, UNATTENDED_SOURCE, &a, Some(&gwk), 2).await;
        assert_eq!(
            row(&pg, &a, 2).await,
            (
                "ticket".into(),
                Some(conversation.clone()),
                Some(ticket.clone())
            )
        );

        // Keys nobody recorded stay the agent's own: a cron's, none at all.
        book(&pg, UNATTENDED_SOURCE, &a, Some("cron_live0001"), 3).await;
        book(&pg, UNATTENDED_SOURCE, &a, None, 4).await;
        assert_eq!(
            row(&pg, &a, 3).await,
            (UNATTENDED_SOURCE.into(), None, None)
        );
        assert_eq!(
            row(&pg, &a, 4).await,
            (UNATTENDED_SOURCE.into(), None, None)
        );

        // Both records are the agent's: another agent sending the same key
        // claims nothing.
        book(&pg, UNATTENDED_SOURCE, &other, Some(&gwk), 5).await;
        assert_eq!(
            row(&pg, &other, 5).await,
            (UNATTENDED_SOURCE.into(), None, None)
        );

        // An operator key's row keeps its own source whatever key it names.
        book(&pg, RELAY_SOURCE, &a, None, 6).await;
        assert_eq!(row(&pg, &a, 6).await, (RELAY_SOURCE.into(), None, None));

        // A later flow on the same conversation wins for the rows after it.
        let channel = uuid();
        record_attribution(&flow(&pg, &a, "channel", &channel, None), &conversation)
            .await
            .unwrap();
        book(&pg, UNATTENDED_SOURCE, &a, Some(&gwk), 7).await;
        assert_eq!(
            row(&pg, &a, 7).await,
            ("channel".into(), Some(channel), None)
        );
        cleanup(&pg, &a).await;
        cleanup(&pg, &other).await;
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn the_second_record_backfills_only_the_last_days_unattended_rows() {
        let pg = pg().await;
        let a = agent();
        cleanup(&pg, &a).await;
        let (conversation, run) = (uuid(), uuid());
        let gwk = format!("gwk_{}", &uuid()[..8]);

        // Booked before either record: two calls of this turn, one under the
        // same key two days ago, and one row some other flow already owns.
        book(&pg, UNATTENDED_SOURCE, &a, Some(&gwk), 11).await;
        book(&pg, UNATTENDED_SOURCE, &a, Some(&gwk), 12).await;
        book(&pg, UNATTENDED_SOURCE, &a, Some(&gwk), 13).await;
        sqlx::query(
            "update usage_events set created_at = now() - interval '2 days' \
              where agent_model = $1 and prompt_tokens = 13",
        )
        .bind(&a)
        .execute(&pg)
        .await
        .unwrap();
        book(&pg, RELAY_SOURCE, &a, Some(&gwk), 14).await;

        // The transport's record alone cannot reach rows under a key it
        // has never heard of.
        record_attribution(&flow(&pg, &a, "research", &run, None), &conversation)
            .await
            .unwrap();
        assert_eq!(
            backfill_attribution(&pg, &a, &conversation).await.unwrap(),
            0
        );

        // The plugin's report lands second and claims this turn's two calls.
        assert_eq!(
            record_affinity_key(&pg, &a, &gwk, &conversation)
                .await
                .unwrap(),
            2
        );
        for prompt in [11, 12] {
            assert_eq!(
                row(&pg, &a, prompt).await,
                ("research".into(), Some(run.clone()), None)
            );
        }
        assert_eq!(
            row(&pg, &a, 13).await,
            (UNATTENDED_SOURCE.into(), None, None),
            "a row older than a day is never reached back over"
        );
        assert_eq!(
            row(&pg, &a, 14).await,
            (RELAY_SOURCE.into(), None, None),
            "a row that is not unattended is left alone"
        );
        // Run again, it finds nothing left to claim.
        assert_eq!(
            backfill_attribution(&pg, &a, &conversation).await.unwrap(),
            0
        );

        // The completions arm's order: calls booked under the conversation's
        // own id, then the transport records it from the response headers.
        let mut completions = format!("api-{}", uuid().replace('-', ""));
        completions.truncate(20);
        let chat = uuid();
        book(&pg, UNATTENDED_SOURCE, &a, Some(&completions), 15).await;
        record_attribution(&flow(&pg, &a, "chat", &chat, None), &completions)
            .await
            .unwrap();
        assert_eq!(
            backfill_attribution(&pg, &a, &completions).await.unwrap(),
            1
        );
        assert_eq!(row(&pg, &a, 15).await, ("chat".into(), Some(chat), None));
        cleanup(&pg, &a).await;
    }

    /// An app's rows: booked under the caller stamp with source `app`, the
    /// run's stated ref and ticket kept when they are uuids and dropped —
    /// never the row with them — when they are not; and the app's budget
    /// subject sums every stamp under it, and nothing beside it.
    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn an_app_books_under_its_stamps_and_its_budget_sums_them() {
        let pg = pg().await;
        let slug = format!("live-{}", &uuid()[..8]);
        let own = crate::app_keys::app_key_name(&slug);
        let (triage, fitness, neighbour) = (
            format!("{own}:triage"),
            format!("{own}:fitness:triage"),
            format!("{own}x"),
        );
        for name in [&own, &triage, &fitness, &neighbour] {
            cleanup(&pg, name).await;
        }
        let (refd, ticket) = (uuid(), uuid());
        let counts = |prompt: i64| TokenCounts {
            prompt_tokens: prompt,
            completion_tokens: 1,
            ..TokenCounts::default()
        };
        let booked = |name: String, prompt: i64, stated: StatedFlow| {
            let pg = pg.clone();
            async move {
                record_relay_usage(
                    &pg,
                    crate::app_keys::APP_SOURCE,
                    &name,
                    None,
                    &stated,
                    "live-test-endpoint",
                    "local",
                    "live-test-model",
                    &counts(prompt),
                    false,
                    None,
                )
                .await
                .expect("the relay's insert");
            }
        };
        booked(
            triage.clone(),
            21,
            StatedFlow {
                ref_id: Some(refd.clone()),
                task_id: Some(ticket.clone()),
            },
        )
        .await;
        booked(
            fitness.clone(),
            22,
            StatedFlow {
                ref_id: Some("msg-41".into()),
                task_id: Some("t-41".into()),
            },
        )
        .await;
        booked(own.clone(), 23, StatedFlow::default()).await;
        booked(neighbour.clone(), 1000, StatedFlow::default()).await;

        assert_eq!(
            row(&pg, &triage, 21).await,
            ("app".into(), Some(refd), Some(ticket))
        );
        assert_eq!(
            row(&pg, &fitness, 22).await,
            ("app".into(), None, None),
            "a ref that is not a uuid leaves the row, not the ledger"
        );
        let spend = spend_since(&pg, 24, Some(&own))
            .await
            .expect("the app window");
        assert_eq!(spend.tokens, 21 + 22 + 23 + 3, "every stamp under the app");
        let one = spend_since(&pg, 24, Some(&triage))
            .await
            .expect("one stamp");
        assert_eq!(one.tokens, 22, "a stamp is its own name alone");
        for name in [&own, &triage, &fitness, &neighbour] {
            cleanup(&pg, name).await;
        }
    }
}

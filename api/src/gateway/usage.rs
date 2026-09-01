// Token ledger — port of ui/src/server/usage.ts: usage normalization across
// every provider shape, the chars/4 estimate, the priced rolling-window spend
// read (budget check's read side), and the gateway's usage_events insert.

use crate::body::js_num;
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
// so they can be surfaced as "unpriced". Multipliers per input KIND — TS
// interpolates the constants into its unsafe string, and so does this: they
// are VALUES in the text, not binds (an earlier draft bound them, which
// collided with every caller's own $2 and made the statement unpreparable —
// a failure budget.rs's `.ok()?` swallowed as "no spend data").
static PRICED_STR: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "select u.*, \
        case \
          when u.endpoint_class = 'local' then 0 \
          when u.endpoint_class = 'cloud' then \
            ((u.prompt_tokens + u.cache_write_tokens * {CACHE_WRITE_MULTIPLIER} + u.cache_read_tokens * {CACHE_READ_MULTIPLIER}) \
           * coalesce((e.model_prices->u.llm_model->>'in')::numeric, \
                      (e.auto_prices->u.llm_model->>'in')::numeric, e.price_in_per_mtok) \
         + u.completion_tokens * coalesce((e.model_prices->u.llm_model->>'out')::numeric, \
                                          (e.auto_prices->u.llm_model->>'out')::numeric, e.price_out_per_mtok)) / 1e6 \
      else null \
    end as cost \
  from usage_events u \
  left join llm_endpoints e on e.name = u.endpoint"
    )
});

/// The priced view as &str, for format! interpolation.
fn priced() -> &'static str {
    &PRICED_STR
}

/// Billable spend over a rolling window, optionally for one caller
/// (`agent_model` — a fleet model id for persona rows, `api:<key>` for
/// gateway rows). Port of spendSince; called before every budgeted call and
/// cached by budget.rs.
pub async fn spend_since(
    pg: &PgPool,
    window_hours: i64,
    agent_model: Option<&str>,
) -> Result<SpendWindow, sqlx::Error> {
    let priced = priced();
    let hours = window_hours.clamp(1, 24 * 365);
    // sqlx can't interpolate the interval literal as a bind for `now() - $1`
    // with unit attached — build `make_interval(hours => $1)` instead, which
    // stays a bound parameter the way TS's unsafe string does not.
    let sql = format!(
        "with priced as ({priced}) \
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

// ── The token ledger overview (usage.ts costOverview → GET /api/cost) ────────

/// One window's aggregate, in wire order (usage.ts's `t()` shape).
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
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPerModel {
    llm_model: Option<String>,
    endpoint_class: Option<String>,
    tokens: i64,
    /// Null when every row for the model is unpriced — shown, never silent $0.
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
    /// 0..1 of this agent's attributed tokens served locally.
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

/// The token ledger overview (usage.ts costOverview): totals, per-model,
/// per-agent, per-day — the Observability cost page's whole read. The nine
/// aggregates are independent; TS fans them out concurrently and so does
/// this (alerts + /cost call it on every load).
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
                            last_used: last_ms.map(crate::agent_auth::epoch_ms_to_iso),
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
    let w =
        |(prompt, completion, cache, generations, cost): (i32, i32, i32, i32, f64)| CostWindow {
            prompt,
            completion,
            cache,
            generations,
            cost: js_num(cost),
        };
    Ok(CostOverview {
        totals: CostTotals {
            today: w(today),
            week: w(week),
            month: w(month),
            // 0..1 of the month's generations that are estimates.
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
        },
        per_model,
        per_agent,
        per_day,
    })
}

// ── Ticket usage (usage.ts taskUsage → the tasks family's detail read) ───────

/// One per-model line of a ticket's usage.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsagePerModel {
    pub llm_model: Option<String>,
    pub tokens: i64,
    /// Null when every row for the model is unpriced — shown, never silent $0.
    pub cost: Option<serde_json::Number>,
}

/// Token spend reported against one ticket (usage.ts TaskUsage). Agents
/// self-report it via MCP log_usage for work done outside Talaria's request
/// path — by design it adds to the same totals.
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
    // stays a bind, exactly as TS's sql.unsafe($1) does.
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

// ── Agent-reported spend (usage.ts recordUsage → tasks.$id.usage POST) ───────

/// Which model serves this generation: the requested TIER's model when a tier
/// was routed, else the agent's current MAIN model.
///
/// The agent's stored `endpoint` is a config-time PREFERENCE, not what ran:
/// agents call the gateway by model name and `resolve_route` round-robins the
/// pool serving it, so trusting the config can stamp a cloud turn `local` and
/// price it at $0. Classify from the gateway's real pool instead and record
/// only what's certain — one server is exact, a pool that agrees on class is
/// priced by class, a mixed pool leaves the row unattributed rather than
/// guessing.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentClass {
    pub endpoint_class: Option<String>,
    pub llm_model: String,
    pub endpoint: Option<String>,
}

type ClassCache = std::collections::HashMap<String, (std::time::Instant, Option<AgentClass>)>;

static CLASS_CACHE: std::sync::LazyLock<std::sync::Mutex<ClassCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(ClassCache::new()));

/// classifyAgent — the serving tier-or-main spec for an agent, narrowed to
/// what the gateway's pool can honestly claim. Cached 60s per
/// (agentModel, tier): the def and the endpoint pool both move slower than
/// that, and the hot path should never feel a classification.
pub async fn classify_agent(
    pg: &PgPool,
    agent_model: &str,
    tier: Option<&str>,
) -> Result<Option<AgentClass>, sqlx::Error> {
    let cache_key = format!("{agent_model}:{}", tier.unwrap_or(""));
    {
        let cache = CLASS_CACHE.lock().unwrap();
        if let Some((at, value)) = cache.get(&cache_key)
            && at.elapsed().as_secs() < 60
        {
            return Ok(value.clone());
        }
    }
    let row: Option<(Option<String>, Option<String>, Option<String>)> = match tier {
        Some(tier) => {
            sqlx::query_as(
                "select e.class, (a->>'model') as model, (a->>'endpoint') as endpoint \
                 from agent_defs d \
                 join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
                 cross join lateral jsonb_array_elements(coalesce(v.config->'aliases','[]'::jsonb)) a \
                 left join llm_endpoints e on e.name = (a->>'endpoint') \
                 where d.model = $1 and a->>'name' = $2",
            )
            .bind(agent_model)
            .bind(tier)
            .fetch_optional(pg)
            .await?
        }
        None => {
            sqlx::query_as(
                "select e.class, (v.config->'main'->>'model') as model, (v.config->'main'->>'endpoint') as endpoint \
                 from agent_defs d \
                 join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
                 left join llm_endpoints e on e.name = (v.config->'main'->>'endpoint') \
                 where d.model = $1",
            )
            .bind(agent_model)
            .fetch_optional(pg)
            .await?
        }
    };
    let value = match row {
        // A tier still pointed at a legacy upstream (no model on the alias)
        // classifies as nothing — an unattributable row, never a guess.
        Some((class, Some(model), endpoint)) if !model.is_empty() => {
            classify_model(pg, &model, (class.as_deref(), endpoint.as_deref())).await?
        }
        _ => None,
    };
    CLASS_CACHE
        .lock()
        .unwrap()
        .insert(cache_key, (std::time::Instant::now(), value.clone()));
    Ok(value)
}

/// The serving endpoint as the GATEWAY would pick it, narrowed to what we can
/// honestly claim. `configured` is the agent's stored spec — the only thing
/// left to go on for a model the gateway doesn't serve (a tier still pointed
/// at a legacy upstream).
async fn classify_model(
    pg: &PgPool,
    model: &str,
    configured: (Option<&str>, Option<&str>),
) -> Result<Option<AgentClass>, sqlx::Error> {
    let (class, endpoint) = configured;
    let routing = crate::gateway::registry::routing_for(pg, model).await?;
    if routing.endpoints.len() == 1 {
        let one = &routing.endpoints[0];
        return Ok(Some(AgentClass {
            endpoint_class: Some(one.class.clone()),
            llm_model: routing.upstream_model,
            endpoint: Some(one.name.clone()),
        }));
    }
    if routing.endpoints.is_empty() {
        return Ok(class.map(|class| AgentClass {
            endpoint_class: Some(class.to_string()),
            llm_model: model.to_string(),
            endpoint: endpoint.map(str::to_string),
        }));
    }
    // A pool: the class is certain when every member agrees (local stays $0,
    // cloud stays visible as unpriced), the endpoint never is.
    let all_agree = routing
        .endpoints
        .iter()
        .map(|e| e.class.as_str())
        .all(|c| c == routing.endpoints[0].class.as_str());
    Ok(Some(AgentClass {
        endpoint_class: all_agree.then(|| routing.endpoints[0].class.clone()),
        llm_model: routing.upstream_model,
        endpoint: None,
    }))
}

/// One agent-reported spend row (usage.ts UsageInput). 'chat'/'channel' rows
/// are gateway-metered by Talaria; 'ticket' rows are agent-SELF-REPORTED (MCP
/// log_usage) for work done outside Talaria's request path — by design they
/// add to the same totals (that spend is just as real), guarded by the agent
/// key + board policy rather than metering.
pub struct UsageInput<'a> {
    pub agent_model: &'a str,
    pub source: &'a str,
    pub ref_id: Option<&'a str>,
    pub task_id: Option<&'a str>,
    /// Alias tier the request was routed to (None = the agent's main model).
    pub tier: Option<&'a str>,
    pub counts: TokenCounts,
    pub estimated: bool,
}

/// recordUsage — insert one usage_events row, attributed by the classify
/// plane. Classification failure is swallowed to null here (TS:
/// `.catch(() => null)`): the row still lands, unattributed.
pub async fn record_usage(pg: &PgPool, u: &UsageInput<'_>) -> Result<(), sqlx::Error> {
    let cls = classify_agent(pg, u.agent_model, u.tier)
        .await
        .unwrap_or(None);
    sqlx::query(
        "insert into usage_events (agent_model, source, ref_id, task_id, prompt_tokens, completion_tokens, \
                                  cache_write_tokens, cache_read_tokens, reasoning_tokens, \
                                  estimated, endpoint_class, llm_model, endpoint) \
         values ($1, $2, $3::uuid, $4::uuid, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(u.agent_model)
    .bind(u.source)
    .bind(u.ref_id)
    .bind(u.task_id)
    .bind(k(u.counts.prompt_tokens))
    .bind(k(u.counts.completion_tokens))
    .bind(k(u.counts.cache_write_tokens))
    .bind(k(u.counts.cache_read_tokens))
    .bind(k(u.counts.reasoning_tokens))
    .bind(u.estimated)
    .bind(cls.as_ref().and_then(|c| c.endpoint_class.clone()))
    .bind(cls.as_ref().map(|c| c.llm_model.clone()))
    .bind(cls.as_ref().and_then(|c| c.endpoint.clone()))
    .execute(pg)
    .await
    .map(|_| ())?;

    // A cloud row landing without a price is the oracle's cue to look again —
    // detached, throttled, and idempotent, so the hot path never feels it. A
    // probe that itself fails nudges nothing (TS: `.catch(() => {})`).
    if let Some(cls) = cls
        && cls.endpoint_class.as_deref() == Some("cloud")
        && cls.endpoint.is_some()
        && !cls.llm_model.is_empty()
    {
        let pg = pg.clone();
        let endpoint = cls.endpoint.clone().unwrap_or_default();
        let model = cls.llm_model.clone();
        tokio::spawn(async move {
            let priced = sqlx::query(
                "select 1 as ok from llm_endpoints \
                 where name = $1 \
                   and (model_prices ? $2 or auto_prices ? $2 or price_in_per_mtok is not null)",
            )
            .bind(&endpoint)
            .bind(&model)
            .fetch_optional(&pg)
            .await;
            if matches!(priced, Ok(None)) {
                crate::price_oracle::nudge_auto_prices(&pg);
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_numbers_print_like_json_stringify() {
        // Integral floats lose the ".0" — JSON.stringify(0) is "0".
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

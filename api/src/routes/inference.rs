// /api/inference — port of ui/src/routes/api/inference.ts.
// Local inference: your own hardware's backends (class=local), probed live,
// plus what they've served from the token ledger. Config lives on /models.

use crate::error::thrown_internal_error;
use crate::fleet_docker::{Health, container_status};
use crate::gateway::provider::catalog_models;
use crate::gateway::registry::list_endpoints;
use crate::gateway::upstream::gateway_pulse;
use crate::session::require_view;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::time::Instant;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Backend base URLs + org-wide usage: admins + Observability grantees.
    if let Err(gate) = require_view(&state, &headers, "/observability").await {
        return gate;
    }

    let endpoints = match list_endpoints(&state.pg).await {
        Ok(eps) => eps,
        Err(e) => {
            tracing::error!("[inference] endpoint read failed: {e}");
            return thrown_internal_error();
        }
    };
    // Each probe runs concurrently, as the TS Promise.all did.
    let probes = endpoints
        .iter()
        .filter(|ep| ep.class == "local")
        .map(|ep| {
            let state = state.clone();
            let ep = ep.clone();
            async move {
                let started = Instant::now();
                let health = match catalog_models(&state, &ep).await {
                    Ok(serving) => json!({
                        "ok": true,
                        "latencyMs": started.elapsed().as_millis() as i64,
                        "servingNow": serving.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                        "note": Value::Null,
                    }),
                    Err(note) => json!({
                        "ok": false,
                        "latencyMs": Value::Null,
                        "servingNow": [],
                        "note": note,
                    }),
                };
                json!({
                    "id": ep.id,
                    "name": ep.name,
                    "baseUrl": ep.base_url,
                    "models": ep.models,
                    "health": health,
                })
            }
        });
    let backends: Vec<Value> = futures_util::future::join_all(probes).await;

    let pg = &state.pg;
    // ── Ledger totals: today and the trailing month ────────────────────────
    let totals: (i64, i64, i32) = match sqlx::query_as(
        "select coalesce(sum(prompt_tokens + completion_tokens) filter (where created_at > now() - interval '1 day'), 0)::bigint as today, \
                coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as month, \
                count(*)::int as generations \
         from usage_events \
         where endpoint_class = 'local' and created_at > now() - interval '30 days'",
    )
    .fetch_one(pg)
    .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[inference] usage totals failed: {e}");
            return thrown_internal_error();
        }
    };
    let per_model: Vec<(Option<String>, i64)> = match sqlx::query_as(
        "select llm_model as \"llmModel\", coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as tokens \
         from usage_events \
         where endpoint_class = 'local' and created_at > now() - interval '30 days' \
         group by llm_model order by tokens desc",
    )
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[inference] per-model usage failed: {e}");
            return thrown_internal_error();
        }
    };

    // ── Live pulse: what's generating right now + the last hour ─────────
    // Streaming rows persist throttled during a reply; a crashed stream
    // stays 'streaming' forever, hence the 10-minute recency clamp.
    let generating: Vec<(String, i32)> = match sqlx::query_as(
        "select c.agent_model as \"agentModel\", count(*)::int as count \
         from messages m join conversations c on c.id = m.conversation_id \
         where m.status = 'streaming' and m.created_at > now() - interval '10 minutes' \
         group by 1 \
         union all \
         select cm.author as \"agentModel\", count(*)::int as count \
         from channel_messages cm \
         where cm.status = 'streaming' and cm.author_type = 'agent' \
           and cm.created_at > now() - interval '10 minutes' \
         group by 1",
    )
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[inference] generating read failed: {e}");
            return thrown_internal_error();
        }
    };
    // `lastAt` is a Date in TS and serializes as Date.toJSON(): UTC with
    // exactly three fraction digits.
    let last_hour: Vec<(String, i32, i64, String)> = match sqlx::query_as(
        "select agent_model as \"agentModel\", count(*)::int as generations, \
                coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as tokens, \
                to_char(max(created_at) at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') as \"lastAt\" \
         from usage_events \
         where created_at > now() - interval '1 hour' \
         group by 1 order by tokens desc limit 20",
    )
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[inference] last-hour read failed: {e}");
            return thrown_internal_error();
        }
    };

    // Fleet container temperature: running / warming / unhealthy / down.
    let managed: Vec<(String,)> = match sqlx::query_as(
        "select department from agent_defs where enabled and managed",
    )
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[inference] managed-agent read failed: {e}");
            return thrown_internal_error();
        }
    };
    let departments: Vec<String> = managed.into_iter().map(|(d,)| d).collect();
    let states = if departments.is_empty() {
        Vec::new()
    } else {
        // `.catch(() => [])` — the roster is decoration, not the answer.
        container_status(&departments).await.unwrap_or_default()
    };
    let mut fleet = json!({ "running": 0, "warming": 0, "unhealthy": 0, "down": 0 });
    let fleet_obj = fleet.as_object_mut().expect("the literal is an object");
    for s in &states {
        let bucket = match s.managed.as_ref() {
            None => "down",
            Some(c) if c.state != "running" => "down",
            Some(c) => match c.health {
                Some(Health::Starting) => "warming",
                Some(Health::Unhealthy) => "unhealthy",
                _ => "running",
            },
        };
        if let Some(v) = fleet_obj.get_mut(bucket).and_then(|v| v.as_i64()) {
            fleet_obj.insert(bucket.to_string(), json!(v + 1));
        }
    }

    let pulse = gateway_pulse();
    Json(json!({
        "live": {
            "generating": generating.iter()
                .map(|(agent_model, count)| json!({ "agentModel": agent_model, "count": count }))
                .collect::<Vec<_>>(),
            "lastHour": last_hour.iter()
                .map(|(agent_model, generations, tokens, last_at)| json!({
                    "agentModel": agent_model,
                    "generations": generations,
                    "tokens": tokens,
                    "lastAt": last_at,
                }))
                .collect::<Vec<_>>(),
            "gateway": {
                "requests": pulse.requests,
                "errors": pulse.errors,
                "p50": pulse.p50,
                "p95": pulse.p95,
            },
            "fleet": fleet,
        },
        "backends": backends,
        "usage": {
            "today": totals.0,
            "month": totals.1,
            "generations": totals.2,
            "perModel": per_model.iter()
                .map(|(llm_model, tokens)| json!({ "llmModel": llm_model, "tokens": tokens }))
                .collect::<Vec<_>>(),
        },
    }))
    .into_response()
}

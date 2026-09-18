// GET /api/fleet/resources?agent=<model>&minutes=<n>. The per-agent
// container resource series the run-detail modal and the Compute panel read:
// cpu/mem/pids sampled once a minute by the agent-resource-sample job.
// Admin. `agent` narrows to one; `minutes` bounds the window (default 60,
// clamped to the table's 7-day retention).

use crate::error::thrown_internal_error;
use crate::session::require_admin;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::collections::HashMap;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let agent = params.get("agent").map(String::as_str);
    let minutes: i32 = params
        .get("minutes")
        .and_then(|m| m.parse().ok())
        .unwrap_or(60)
        .clamp(1, 60 * 24 * 7);
    let rows: Vec<(String, f64, i64, i64, i64)> = match sqlx::query_as(
        "select agent_model, cpu_percent, mem_bytes, pids, \
                (trunc(extract(epoch from taken_at) * 1000))::bigint \
         from agent_resource_samples \
         where taken_at > now() - ($1::int * interval '1 minute') \
           and ($2::text is null or agent_model = $2) \
         order by taken_at asc",
    )
    .bind(minutes)
    .bind(agent)
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[fleet/resources] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut series: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    let mut mem_series: HashMap<String, Vec<i64>> = HashMap::new();
    let mut latest: HashMap<String, serde_json::Value> = HashMap::new();
    for (model, cpu, mem, pids, at_ms) in rows {
        series.entry(model.clone()).or_default().push(json!({
            "t": at_ms,
            "cpu": (cpu * 10.0).round() / 10.0,
            "mem": mem,
            "pids": pids,
        }));
        mem_series.entry(model.clone()).or_default().push(mem);
        latest.insert(
            model.clone(),
            json!({ "cpu": cpu, "mem": mem, "pids": pids, "at": at_ms }),
        );
    }
    let agents: Vec<serde_json::Value> = {
        let mut keys: Vec<&String> = series.keys().collect();
        keys.sort();
        keys.into_iter()
            .map(|k| {
                let leak = crate::fleet::resources::mem_is_leaking(
                    mem_series.get(k).map(Vec::as_slice).unwrap_or(&[]),
                );
                json!({
                    "agent": k,
                    "points": series.get(k).cloned().unwrap_or_default(),
                    "latest": latest.get(k).cloned().unwrap_or(serde_json::Value::Null),
                    "leak": leak,
                })
            })
            .collect()
    };
    let host = crate::fleet::budget::host_mem().await.map(|h| {
        let reserve = crate::fleet::budget::host_reserve_for(h.total);
        json!({
            "total": h.total,
            "available": h.available,
            "reserve": reserve,
            "agentCeiling": crate::fleet::budget::workbench_limit_for(Some(&h)),
            "pressure": crate::fleet::budget::host_pressure(h.available, reserve).as_str(),
        })
    });
    Json(json!({ "host": host, "agents": agents })).into_response()
}

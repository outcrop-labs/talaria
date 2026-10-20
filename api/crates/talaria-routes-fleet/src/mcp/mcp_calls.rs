// /api/mcp/calls, /api/mcp/calls/summary and /api/mcp/calls/retention.
// The gateway's per-call record, read back for admins (`mcp_tool_calls`, one
// row per tools/call or refusal the gateway answered — see mcp::calls).
// GET /api/mcp/calls → `{calls}`, newest first: `?server=` (id or name),
// `agent=` (the fleet model), `tool=`, `outcome=` (ok | error | denied |
// not_connected | upstream_error | spend_error), `since=` (epoch ms) and
// `limit=` (default 100, max 500) narrow it. Each call is `{id, at, serverId,
// serverName, agent, userId, userName, method, tool, outcome, status,
// latencyMs, error}` — never the call's arguments or its result.
// GET /api/mcp/calls/summary → `{since, totals, servers, agents}` over
// `?since=` (epoch ms; default the last 24 hours), optionally narrowed to one
// `server=` (id or name — the server card's one line). Each server is
// `{serverId, serverName, calls, errors, denied, notConnected, p50Ms, p95Ms,
// topTools: [{tool, calls, errors}]}`, each agent `{agent, calls, errors}`;
// errors count error, upstream_error and spend_error, never refusals or a
// caller's cancellation; p50Ms/p95Ms leave out what the gateway answered
// itself (denied, not_connected).
// GET /api/mcp/calls/retention → `{days}`, how long a row is kept (app
// setting `mcp_calls_retention_days`, default 30); PUT `{days}` (a whole
// number, 1-3650) sets it and answers the same shape — the next six-hourly
// prune applies it.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::now_ms;
use talaria_api_facades::mcp::calls::{self, filter_value, parse_filter, parse_since};
use talaria_error::{house_error, internal};
use talaria_session::require_admin;
use talaria_state::AppState;

/// The summary's default window.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

// Every member is a raw string so a bad value answers the house 400 with a
// sentence, not the extractor's plain-text rejection.
#[derive(serde::Deserialize)]
pub struct CallsQuery {
    server: Option<String>,
    agent: Option<String>,
    tool: Option<String>,
    outcome: Option<String>,
    since: Option<String>,
    limit: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallsQuery>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let filter = match parse_filter(
        q.server.as_deref(),
        q.agent.as_deref(),
        q.tool.as_deref(),
        q.outcome.as_deref(),
        q.since.as_deref(),
        q.limit.as_deref(),
    ) {
        Ok(f) => f,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match calls::list_calls(&state.pg, &filter).await {
        Ok(rows) => Ok(Json(json!({ "calls": rows })).into_response()),
        Err(e) => Ok(internal("[mcp/calls] list failed", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct SummaryQuery {
    server: Option<String>,
    since: Option<String>,
}

pub async fn summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SummaryQuery>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let since = match parse_since(q.since.as_deref()) {
        Ok(s) => s.unwrap_or_else(|| now_ms() - DAY_MS),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let server = filter_value(q.server.as_deref());
    match calls::summary(&state.pg, since, server.as_deref()).await {
        Ok(body) => Ok(Json(body).into_response()),
        Err(e) => Ok(internal("[mcp/calls] summary failed", e)),
    }
}

pub async fn retention_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    Ok(Json(json!({ "days": calls::retention(&state.pg).await })).into_response())
}

pub async fn retention_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let asked = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("days").cloned())
        .unwrap_or(Value::Null);
    let days = match calls::parse_retention(&asked) {
        Ok(d) => d,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match calls::set_retention(&state.pg, days).await {
        Ok(()) => Ok(Json(json!({ "days": days })).into_response()),
        Err(e) => Ok(internal("[mcp/calls] retention save failed", e)),
    }
}

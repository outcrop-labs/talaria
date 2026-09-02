// /api/mcp/test.
// POST → probe an MCP server's reachability + auth state (admin only; it
// makes an outbound request to an admin-supplied URL).

use crate::body::{as_object, optional_max_string_member, parse, url_member};
use crate::error::house_error;
use crate::mcp::probe::{McpProbeResult, probe_mcp};
use crate::mcp::service::{mcp_fleet_url, mcp_port};
use crate::session::require_admin;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

fn state_json(r: &McpProbeResult) -> serde_json::Value {
    use crate::mcp::probe::McpProbeState::*;
    match r.state {
        Ok => json!("ok"),
        Auth => json!("auth"),
        Unreachable => json!("unreachable"),
        Error => json!("error"),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let _user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let url = match url_member(obj, "url", 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let agent_slug = match optional_max_string_member(obj, "agentSlug", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // The agent slug → X-Agent-Name header (the fleet's convention), so
    // servers that scope by agent see the right identity while testing.
    let mut send: Vec<(&str, String)> = Vec::new();
    if let Some(slug) = &agent_slug {
        send.push(("X-Agent-Name", slug.clone()));
    }
    // The built-in toolkit endpoint: agents reach it via host.docker.internal
    // (which the host itself can't resolve on Linux) and it requires the
    // fleet key — probe locally, authenticated, without exposing the key.
    // (the header goes out only when the env var is set — an unset key
    // sends no header at all, not an empty one.)
    let url = if url == mcp_fleet_url() {
        if let Ok(key) = std::env::var("TALARIA_AGENT_KEY")
            && !key.is_empty()
        {
            send.push(("X-Api-Key", key));
        }
        format!("http://127.0.0.1:{}/mcp", mcp_port())
    } else {
        url
    };
    let borrowed: Vec<(&str, &str)> = send.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let probe = probe_mcp(&url, borrowed).await;
    Json(json!({ "state": state_json(&probe), "detail": probe.detail })).into_response()
}

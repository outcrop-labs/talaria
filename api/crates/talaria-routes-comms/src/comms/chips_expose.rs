// POST /api/agent/chips — an agent exposes tools as chips on the turn it is
// writing. The platform also calls the same landing path when a tool result
// produces a link or a protected action.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::require_agent;
use talaria_body::parse;
use talaria_chips::surface_tool_chips;
use talaria_error::{internal, object_or_400};
use talaria_state::AppState;

// doc: Expose platform tools as chips on the conversation this agent is in.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let tools = obj
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut chips = Vec::new();
    for tool in tools.into_iter().take(12) {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        chips.push(json!({
            "id": format!("tool:{name}"),
            "kind": "tool",
            "tool": name,
            "label": tool.get("label").and_then(Value::as_str).unwrap_or(name),
            "description": tool.get("description").and_then(Value::as_str).unwrap_or(""),
            "inputs": tool.get("inputs").cloned().unwrap_or(json!({})),
        }));
    }
    let redis = state.redis().await.ok();
    match surface_tool_chips(&state.pg, redis, &caller.model, None, chips).await {
        Ok(_) => Ok(Json(json!({ "ok": true })).into_response()),
        Err(e) => Ok(internal("[chips] expose failed", e)),
    }
}

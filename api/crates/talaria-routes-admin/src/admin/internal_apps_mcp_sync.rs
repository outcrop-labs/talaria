// doc: The app host asks the api to reconcile the registry rows of app-published
// doc: MCP servers with the enabled apps: a row per enabled app with an mcp.ts,
// doc: its catalog copied from the build's load probe, and the row of an app that
// doc: is no longer enabled removed — rolling the agents that carried it. The host
// doc: calls it after a probe writes a catalog and after a failed build turns an
// doc: app off, so a broken app's tools leave the agents that had them. Callable
// doc: only with the root-derived service credential (`x-talaria-internal`); the
// doc: host's proxy never forwards `/api/internal/*`. Answers the app server names
// doc: that remain.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_apps::sync_app_mcp_servers;
use talaria_session::{require_internal, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_internal(&state, &headers)?;
    let sb = secretbox_or_500(&state, "[apps] mcp sync: secretbox").await?;
    let servers = sync_app_mcp_servers(&state.pg, &sb).await;
    Ok(Json(serde_json::json!({ "ok": true, "servers": servers })).into_response())
}

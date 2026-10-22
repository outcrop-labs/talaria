// doc: The app host asks the api to reconcile the registry rows of app-published
// doc: MCP servers with the apps: an enabled app with an mcp.ts gets a serving
// doc: row, its catalog copied from the build's load probe, and the row of an app
// doc: that is turned off, failed its build or lost its mcp.ts is turned off with
// doc: the reason. A row that changes between serving and off rolls the agents
// doc: that carry it, so a broken app's tools leave them and come back when it
// doc: serves again; every agent, team and user grant on the row is kept. An
// doc: admin's own enable/disable of the server is never changed. The host calls
// doc: it after a probe writes a catalog and after a failed build turns an app
// doc: off. Callable only with the root-derived service credential
// doc: (`x-talaria-internal`); the host's proxy never forwards `/api/internal/*`.
// doc: Answers the app server names that serve.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_apps::sync_app_mcp_servers;
use talaria_error::internal;
use talaria_session::{require_internal, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_internal(&state, &headers)?;
    let sb = secretbox_or_500(&state, "[apps] mcp sync: secretbox").await?;
    match sync_app_mcp_servers(&state.pg, &sb).await {
        Ok(servers) => {
            Ok(Json(serde_json::json!({ "ok": true, "servers": servers })).into_response())
        }
        Err(e) => Ok(internal("[apps] mcp sync failed", e)),
    }
}

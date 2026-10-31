// doc: The app host asks the api to reconcile the registry rows of app-published
// doc: MCP servers with the apps: an enabled app with an mcp.ts gets a serving
// doc: row, its catalog copied from the build's load probe, and the row of an app
// doc: that is turned off, failed its build or lost its mcp.ts is turned off with
// doc: the reason. A row that changes between serving and off is applied to the
// doc: agents that carry it, in place and without a restart, so a broken app's
// doc: tools leave them and come back when it serves again; a row whose catalog
// doc: changed tells its carriers to list the tools again. Every agent, team and
// doc: user grant on the row is kept. An admin's own enable/disable of the
// doc: server is never changed. The host calls it after a probe writes a catalog
// doc: and after a failed build turns an app off. The same pass revokes the gateway key of every app that is not
// doc: enabled — the host turns a broken app off by writing the enabled set
// doc: itself. Callable only with the root-derived service credential
// doc: (`x-talaria-internal`); the host's proxy never forwards `/api/internal/*`.
// doc: Answers the app server names that serve.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_apps::{reconcile_app_llm_keys, sync_app_mcp_servers};
use talaria_error::internal;
use talaria_session::{require_internal, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_internal(&state, &headers)?;
    let sb = secretbox_or_500(&state, "[apps] mcp sync: secretbox").await?;
    if let Err(e) = reconcile_app_llm_keys(&state.pg).await {
        tracing::warn!("[apps] revoking the gateway keys of apps that are off failed: {e}");
    }
    match sync_app_mcp_servers(&state.pg, &sb).await {
        Ok(servers) => {
            Ok(Json(serde_json::json!({ "ok": true, "servers": servers })).into_response())
        }
        Err(e) => Ok(internal("[apps] mcp sync failed", e)),
    }
}

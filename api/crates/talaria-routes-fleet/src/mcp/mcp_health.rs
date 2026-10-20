// /api/mcp/health.
// POST → run one MCP health pass now (admin only) and answer its report:
// `{checked, changed, down, recovered, notified, servers: [{id, name, state,
// detail, event, catalogChanged}]}`. The same pass the hourly `mcp-health`
// job runs — every enabled external server re-probed, tool catalogs
// refreshed when their names changed, stale assignment subsets recorded,
// and one notification per admin for each server that just went from ok to
// down. It makes outbound requests to every registered URL, hence admin.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_api_facades::mcp::health::run_health_pass;
use talaria_error::house_error;
use talaria_notify::NotifyDeps;
use talaria_session::{require_admin, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let _user = require_admin(&state, &headers).await?;
    let sb = secretbox_or_500(&state, "[mcp/health] secretbox unavailable").await?;
    // The realtime half of the notification write: the bell on an open page
    // learns of a filed row the moment it lands, as with every other writer.
    let realtime = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()).realtime;
    match run_health_pass(&state.pg, &sb, &realtime, talaria_agent_auth::now_ms()).await {
        Ok(report) => Ok(Json(report).into_response()),
        Err(e) => Ok(house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the health pass could not read the registry: {e}"),
        )),
    }
}

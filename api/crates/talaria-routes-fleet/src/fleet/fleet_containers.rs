// GET /api/fleet/containers. Container reality per agent (the managed
// service), admin.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::docker::container_status;
use talaria_error::internal;
use talaria_session::require_admin;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let rows: Result<Vec<(String,)>, sqlx::Error> =
        sqlx::query_as("select department from agent_defs where enabled order by slug")
            .fetch_all(&state.pg)
            .await;
    let departments = match rows {
        Ok(r) => r.into_iter().map(|(d,)| d).collect::<Vec<_>>(),
        Err(e) => return internal("[fleet] fleet_containers failed", e),
    };
    // container_status errors on a docker failure — the whole route 500s
    // the house way (no json body).
    match container_status(&departments).await {
        Ok(containers) => Json(json!({ "containers": containers })).into_response(),
        Err(e) => internal("[fleet] container_status failed", e),
    }
}

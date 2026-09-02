// GET /api/fleet/containers — port of ui/src/routes/api/fleet.containers.ts.
// Container reality per agent (the managed service), admin.

use crate::error::thrown_internal_error;
use crate::fleet::docker::container_status;
use crate::session::require_admin;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

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
        Err(_) => return thrown_internal_error(),
    };
    // containerStatus throws on a docker failure — the whole route 500s the
    // house way (no json body), same as the TS throw.
    match container_status(&departments).await {
        Ok(containers) => Json(json!({ "containers": containers })).into_response(),
        Err(_) => thrown_internal_error(),
    }
}

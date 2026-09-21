// /api/gaps. GET → the Studio's Suggested queue: capability gaps agents have reported,
// ranked by how often the work-shape recurs. Any member reads (the queue is
// what invites people to tailor); status changes live on /api/gaps/{id}.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_gaps::list_gaps;
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Deserialize)]
pub struct GapsQuery {
    status: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GapsQuery>,
) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    match list_gaps(&state.pg, query.status.as_deref()).await {
        Ok(gaps) => Json(json!({ "gaps": gaps })).into_response(),
        Err(e) => {
            tracing::error!("[gaps] list failed: {e}");
            talaria_error::thrown_internal_error()
        }
    }
}

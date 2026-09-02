// /api/alerts. GET → derived system alerts (no persistence) for the
// requesting user.

use crate::alerts::compute_alerts;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    Json(json!({ "alerts": compute_alerts(&state, &user.id).await })).into_response()
}

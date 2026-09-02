// GET /api/fleet. Owned fleet ops data (agents + Talaria-native usage).
// Ops-wide detail: admins + people granted the Observability view.

use crate::error::thrown_internal_error;
use crate::fleet::get_fleet_overview;
use crate::session::require_view;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_view(&state, &headers, "/observability").await {
        return gate;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    match get_fleet_overview(&state.pg, now).await {
        Ok(body) => Json(body).into_response(),
        Err(_) => thrown_internal_error(),
    }
}

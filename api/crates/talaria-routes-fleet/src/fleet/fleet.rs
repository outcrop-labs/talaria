// GET /api/fleet. Owned fleet ops data (agents + Talaria-native usage).
// Ops-wide detail: admins + people granted the Observability view.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_api_facades::fleet::get_fleet_overview;
use talaria_error::internal;
use talaria_session::require_view;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_view(&state, &headers, "/observability").await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(match get_fleet_overview(&state.pg, now).await {
        Ok(body) => Json(body).into_response(),
        Err(e) => internal("[fleet] get_fleet_overview failed", e),
    })
}

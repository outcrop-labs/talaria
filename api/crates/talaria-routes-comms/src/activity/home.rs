// /api/home. GET → the Home/Today summary for the signed-in user.
//
// The digest job registers in main.rs's scheduler table, not here — this
// file is only the read.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_error::internal;
use talaria_home::home_summary;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match home_summary(&state, &user.id, user.role == "admin").await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => internal("[home] summary failed", e),
    }
}

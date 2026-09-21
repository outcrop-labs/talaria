// /api/inbox/focus/summary. GET → the one-screen summary of the caller's
// focus state: how many items are queued (snoozed ones excluded), as
// {count}.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::internal;
use talaria_inbox_focus::focus_summary;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match focus_summary(&state.pg, &user).await {
        Ok(count) => (StatusCode::OK, Json(json!({ "count": count }))).into_response(),
        Err(e) => internal("[inbox-focus] summary read failed", e),
    })
}

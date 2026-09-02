// /api/inbox/focus/summary. GET → the one-screen summary of the caller's
// focus state: how many items are queued (snoozed ones excluded), as
// {count}.

use crate::error::thrown_internal_error;
use crate::inbox_focus::focus_summary;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match focus_summary(&state.pg, &user).await {
        Ok(count) => (StatusCode::OK, Json(json!({ "count": count }))).into_response(),
        Err(e) => {
            tracing::error!("[inbox-focus] summary read failed: {e}");
            thrown_internal_error()
        }
    }
}

// /api/inbox/focus. GET → the focus inbox queue: what the assistant has
// teed up for the caller. No options are taken (the queue defaults:
// enrich, no snoozed) — the query string is ignored entirely.

use crate::error::thrown_internal_error;
use crate::inbox_focus::{FocusQueueOptions, list_focus_queue};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match list_focus_queue(
        &state,
        &user,
        FocusQueueOptions {
            enrich: true,
            include_snoozed: false,
        },
    )
    .await
    {
        Ok(queue) => (StatusCode::OK, Json(queue)).into_response(),
        Err(e) => {
            tracing::error!("[inbox-focus] queue read failed: {e}");
            thrown_internal_error()
        }
    }
}

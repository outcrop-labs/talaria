// /api/inbox/focus. GET → the focus inbox queue: what the assistant has
// teed up for the caller. No options are taken (the queue defaults:
// enrich, no snoozed) — the query string is ignored entirely.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_error::internal;
use talaria_inbox_focus::{FocusQueueOptions, list_focus_queue};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(
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
            Err(e) => internal("[inbox-focus] queue read failed", e),
        },
    )
}

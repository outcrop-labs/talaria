// /api/inbox/focus — port of ui/src/routes/api/inbox.focus.ts.
// GET → the focus inbox queue: what the assistant has teed up for the caller.
// TS takes no options here (the queue defaults: enrich, no snoozed), and the
// query string is ignored entirely.

use crate::error::house_error;
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
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

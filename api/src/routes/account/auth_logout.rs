// POST /api/auth/logout. Delete the Redis session and clear the cookie.

use crate::error::thrown_internal_error;
use crate::session::{clear_session_cookie, destroy_session, json_with_cookies};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

#[derive(serde::Serialize)]
struct OkBody {
    ok: bool,
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = destroy_session(&state, &headers).await {
        tracing::error!("[auth/logout] redis delete failed: {e}");
        return thrown_internal_error();
    }
    json_with_cookies(Json(OkBody { ok: true }), &[clear_session_cookie()])
}

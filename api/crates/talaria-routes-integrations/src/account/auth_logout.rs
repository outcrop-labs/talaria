// POST /api/auth/logout. Delete the Redis session and clear the cookie.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use talaria_error::internal;
use talaria_session::{clear_session_cookie_for, destroy_session, json_with_cookies};
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct OkBody {
    ok: bool,
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = destroy_session(&state, &headers).await {
        return internal("[auth/logout] redis delete failed", e);
    }
    json_with_cookies(
        Json(OkBody { ok: true }),
        &[clear_session_cookie_for(&headers)],
    )
}

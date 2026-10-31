// POST /api/auth/logout. Delete the Redis session and clear the cookie — and,
// first, every push subscription this session filed: a browser that signed
// out must stop receiving the signed-out person's notifications. Rows filed
// by the person's OTHER sessions (another browser, another device) stay.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use talaria_error::internal;
use talaria_session::{
    clear_session_cookie_for, destroy_session, json_with_cookies, session_hash_of,
};
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct OkBody {
    ok: bool,
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Before the session dies: the hash names it. A failed delete is logged
    // and the sign-out still happens — the client unsubscribed its own
    // browser already, and leaving a person signed in is the worse outcome.
    if let Some(hash) = session_hash_of(&headers)
        && let Err(e) = sqlx::query("delete from push_subscriptions where session_hash = $1")
            .bind(&hash)
            .execute(&state.pg)
            .await
    {
        tracing::error!("[auth/logout] could not retire this session's push subscriptions: {e}");
    }
    if let Err(e) = destroy_session(&state, &headers).await {
        return internal("[auth/logout] redis delete failed", e);
    }
    json_with_cookies(
        Json(OkBody { ok: true }),
        &[clear_session_cookie_for(&headers)],
    )
}

// PUT /api/me/presence. The signed-in person's heartbeat: the client calls it
// every 30s while a Talaria tab is visible, and it sets
// `user:presence:{id}` with a 90s TTL. `GET /api/users` reads those keys to
// mark people online. Answers { ok: true }.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_users::{PRESENCE_TTL_S, presence_key};

pub async fn put(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[me/presence] redis unavailable", e)),
    };
    if let Err(e) = redis::cmd("SET")
        .arg(presence_key(&user.id))
        .arg(1)
        .arg("EX")
        .arg(PRESENCE_TTL_S)
        .query_async::<()>(&mut conn)
        .await
    {
        return Ok(internal("[me/presence] ping failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

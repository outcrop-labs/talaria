// POST /api/notifications/ack. A client SURFACED these rows to the person —
// toasted them to someone looking, or posted an OS banner (I-18) — and says
// so: `{ids: uuid[] ≤ 200}` → `{ok, acked}`. Only the caller's own rows are
// touched and the first ack wins (`surfaced_at`). W2-E's deferred push fires
// only for rows still unread and un-acked when they come due.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{parse, uuid_array_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// The most ids one ack carries — the mark-read door's own bound.
const ACK_MAX: usize = 200;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let ids = match uuid_array_member(obj, "ids", ACK_MAX) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let acked = match talaria_notify::defer::ack(&state.pg, &user.id, &ids).await {
        Ok(n) => n,
        Err(e) => return Ok(internal("[notifications/ack] the ack write failed", e)),
    };
    Ok(Json(json!({ "ok": true, "acked": acked })).into_response())
}

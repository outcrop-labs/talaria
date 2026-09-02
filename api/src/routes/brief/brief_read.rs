// /api/brief/read. POST { briefId, seq } → move the brief reader's cursor.
// The ONLY mutation
// this surface exposes — there is no edit, no dismiss and no delete, because
// the document is append-only and every one of those would be a rewrite
// wearing a different name.
//
// Scoped to the caller inside `mark_brief_read` (the update carries
// `user_id`), so a brief id belonging to somebody else matches no row rather
// than moving their cursor.

use crate::body::{NumKind, as_object, number_member, uuid_member};
use crate::daily_brief::mark_brief_read;
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The POST body: briefId (uuid) + seq (integer, ≥0).
struct ReadBody {
    brief_id: String,
    seq: f64,
}

fn validate(obj: &serde_json::Map<String, serde_json::Value>) -> Result<ReadBody, String> {
    let brief_id = uuid_member(obj, "briefId")?;
    // seq: integer, min 0, no max below the safe-integer ceiling (2^53-1).
    // read_seq is int8, so the whole safe range is storable — no int4 clamp.
    let seq = number_member(obj, "seq", NumKind::Int, 0.0, 9_007_199_254_740_991.0)?;
    Ok(ReadBody { brief_id, seq })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = mark_brief_read(&state.pg, &user.id, &body.brief_id, body.seq as i64).await {
        tracing::error!("[brief] cursor move failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

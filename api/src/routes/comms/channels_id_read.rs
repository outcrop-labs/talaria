// /api/channels/{id}/read.
// POST { seq } → advance the caller's read cursor (drives unread badges).

use crate::body::{NumKind, as_object, number_member};
use crate::channels::{channel_role, mark_channel_read};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on read-cursor failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // seq: integer, min 0, no schema max — the ceiling is the safe-integer
    // bound itself.
    let seq = match number_member(obj, "seq", NumKind::Int, 0.0, 9_007_199_254_740_991.0) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // last_read_seq is int4: past 2^31-1 the write would die on Postgres'
    // own overflow (→ 500), so it is refused here.
    if seq > 2_147_483_647.0 {
        tracing::error!("[channels] read cursor past int4: {seq}");
        return thrown_internal_error();
    }
    if let Err(e) = mark_channel_read(&state.pg, &id, &user.id, seq as i32).await {
        tracing::error!("[channels] read-cursor advance failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

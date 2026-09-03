// /api/conversations/{id}/read.
// POST { seq? } → advance the caller's read cursor (drives the thread's
// unread pill). The same contract as /api/channels/{id}/read, gated by the
// conversations' own access rule: the owner, or a member of the plan. An
// absent seq means "the thread's latest" — the context menu's Mark read
// knows WHICH thread, not its seq, and a caller that had to load the thread
// to clear it would be a mark-read that opens what it marks.

use crate::body::{NumKind, as_object, optional_number_member};
use crate::conversations::{conversation_accessible, latest_message_seq, mark_conversation_read};
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
    // The same predicate the GET gates with — the owner, or a plan member.
    // Inaccessible and nonexistent answer the same 403, never leaking which.
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[conversations] access read on read-cursor failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // seq: optional integer, min 0, no schema max — the ceiling is the
    // safe-integer bound itself.
    let seq = match optional_number_member(obj, "seq", NumKind::Int, 0.0, 9_007_199_254_740_991.0) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // last_read_seq is int4: past 2^31-1 the write would die on Postgres'
    // own overflow (→ 500), so it is refused here.
    if let Some(seq) = seq
        && seq > 2_147_483_647.0
    {
        tracing::error!("[conversations] read cursor past int4: {seq}");
        return thrown_internal_error();
    }
    let seq = match seq {
        Some(v) => v as i32,
        // Absent = the thread's own latest, whatever it is right now.
        None => match latest_message_seq(&state.pg, &id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[conversations] latest-seq read on read-cursor failed: {e}");
                return thrown_internal_error();
            }
        },
    };
    if let Err(e) = mark_conversation_read(&state.pg, &id, &user.id, seq).await {
        tracing::error!("[conversations] read-cursor advance failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

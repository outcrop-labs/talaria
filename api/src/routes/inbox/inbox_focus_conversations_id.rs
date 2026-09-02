// /api/inbox/focus/conversations/{id}. One conversation instance, by its
// path id. GET → its timeline page (cursor paginates); DELETE → archive it.
// Both are scoped inside the module to the caller's own inbox
// conversations, so an id from the picker can never touch one of their
// ordinary chats. Archiving, not deleting: the messages and the decision
// timeline are the owner's record of what their assistant did.
//
// GET is unlocked by design: it serves mid-turn. Every read is an MVCC
// snapshot, rows land complete or 'streaming', and the page's `working`
// flag renders the in-flight reply — a 409 here would refuse the panel's
// own polling for the whole length of an assistant turn.

use crate::error::{house_error, thrown_internal_error};
use crate::inbox_focus::conversation::{archive_inbox_conversation, get_inbox_conversation};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(serde::Deserialize)]
pub struct ConversationQuery {
    pub cursor: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ConversationQuery>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    // `current` is the panel's first load, which has no id to name — the
    // server resolves the caller's own latest instance. (DELETE never sees
    // it: no instance is named 'current', so it 404s like any other miss.)
    let requested = if id == "current" {
        None
    } else {
        Some(id.as_str())
    };
    match get_inbox_conversation(&state, &user, query.cursor.as_deref(), requested).await {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        Err(e) => {
            tracing::error!("[inbox-focus] conversation read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match archive_inbox_conversation(&state.pg, &user.id, &id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => house_error(StatusCode::NOT_FOUND, "no such conversation"),
        Err(e) => {
            tracing::error!("[inbox-focus] conversation archive failed: {e}");
            thrown_internal_error()
        }
    }
}

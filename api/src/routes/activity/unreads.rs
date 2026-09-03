// /api/unreads. The rail badges' one read: how much is waiting in each part
// of the app, as { comms, plan, research, notifications }. Comms is rooms
// plus agent chats — both unread planes under one badge — and the counts are
// the SAME predicates the pills themselves ride (list_channels per room,
// list_conversations per thread), so a badge can never disagree with the
// pills it summarizes. Five reads, all counted concurrently; none of them is
// a write, and the partial unread index carries the notifications arms.

use crate::channels::channel_unread_total;
use crate::conversations::conversation_unread_total;
use crate::error::thrown_internal_error;
use crate::notify::{unread_count, unread_count_of_kind};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let (rooms, chats, plans, research, bell) = tokio::join!(
        channel_unread_total(&state.pg, &user.id),
        conversation_unread_total(&state.pg, &user.id, "chat"),
        conversation_unread_total(&state.pg, &user.id, "plan"),
        unread_count_of_kind(&state.pg, &user.id, "research"),
        unread_count(&state.pg, &user.id),
    );
    // A badge that silently reads 0 because one arm failed is the unread-null
    // doctrine's one rule backwards: the client renders `!` on failure, and
    // it can only do that if a failure is a failure.
    let (rooms, chats) = match (rooms, chats) {
        (Ok(r), Ok(c)) => (r, c),
        _ => {
            tracing::error!("[unreads] comms arms failed");
            return thrown_internal_error();
        }
    };
    let plans = match plans {
        Ok(v) => v,
        Err(_) => {
            tracing::error!("[unreads] plan arm failed");
            return thrown_internal_error();
        }
    };
    let research = match research {
        Ok(v) => v,
        Err(_) => {
            tracing::error!("[unreads] research arm failed");
            return thrown_internal_error();
        }
    };
    let bell = match bell {
        Ok(v) => v,
        Err(_) => {
            tracing::error!("[unreads] notifications arm failed");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "comms": rooms + chats,
        "plan": plans,
        "research": research,
        "notifications": bell,
    }))
    .into_response()
}

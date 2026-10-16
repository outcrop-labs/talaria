// /api/unreads. The rail badges' one read: how much is waiting in each part
// of the app, as { comms, plan, work, research, notifications }. Comms is
// rooms plus agent chats — both unread planes under one badge — and the
// counts are the SAME predicates the pills themselves ride (list_channels per
// room, list_conversations per thread), so a badge can never disagree with
// the pills it summarizes. Six reads, all counted concurrently; none of them
// is a write, and the partial unread index carries the notifications arms.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_channels::channel_unread_total;
use talaria_conversations::conversation_unread_total;
use talaria_error::internal;
use talaria_notify::{unread_count, unread_count_of_kind};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // A work session is a conversation like a plan is, so its badge is the
    // same read with a different kind — `conversation_unread_total` already
    // scopes the shared kinds by membership, which is what makes a session
    // someone else started and shared with you count here at all.
    let (rooms, chats, plans, work, research, bell) = tokio::join!(
        channel_unread_total(&state.pg, &user.id),
        conversation_unread_total(&state.pg, &user.id, "chat"),
        conversation_unread_total(&state.pg, &user.id, "plan"),
        conversation_unread_total(&state.pg, &user.id, "work"),
        unread_count_of_kind(&state.pg, &user.id, "research"),
        unread_count(&state.pg, &user.id),
    );
    // A badge that silently reads 0 because one arm failed is the unread-null
    // doctrine's one rule backwards: the client renders `!` on failure, and
    // it can only do that if a failure is a failure.
    let (rooms, chats) = match (rooms, chats) {
        (Ok(r), Ok(c)) => (r, c),
        _ => return Ok(internal("[unreads]", "comms arms failed")),
    };
    let plans = match plans {
        Ok(v) => v,
        Err(_) => return Ok(internal("[unreads]", "plan arm failed")),
    };
    let work = match work {
        Ok(v) => v,
        Err(_) => return Ok(internal("[unreads]", "work arm failed")),
    };
    let research = match research {
        Ok(v) => v,
        Err(_) => return Ok(internal("[unreads]", "research arm failed")),
    };
    let bell = match bell {
        Ok(v) => v,
        Err(_) => return Ok(internal("[unreads]", "notifications arm failed")),
    };
    Ok(Json(json!({
        "comms": rooms + chats,
        "plan": plans,
        "work": work,
        "research": research,
        "notifications": bell,
    }))
    .into_response())
}

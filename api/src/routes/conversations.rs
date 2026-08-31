// /api/conversations — port of ui/src/routes/api/conversations.ts. GET
// ?kind=plan → the user's plan conversations; anything else → their chats.
// Newest activity first; the client groups them by agent.

use crate::conversations::list_conversations;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct ConversationsEnvelope {
    conversations: Vec<crate::conversations::ConversationListRow>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    // searchParams.get('kind') === 'plan' ? 'plan' : 'chat'
    let kind = uri
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(key, _)| key == "kind")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    let kind = if kind == "plan" { "plan" } else { "chat" };
    match list_conversations(&state.pg, &user.id, kind).await {
        Ok(rows) => (
            StatusCode::OK,
            Json(ConversationsEnvelope {
                conversations: rows,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[conversations] list failed: {e}");
            crate::error::house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

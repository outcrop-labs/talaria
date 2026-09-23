// /api/conversations. GET
// ?kind=plan → the user's plan conversations; anything else → their chats.
// ?archived=1 → the retired set (exact string '1'); everything else is live.
// Newest activity first; the client groups them by agent.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use talaria_conversations::list_conversations;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct ConversationsEnvelope {
    conversations: Vec<talaria_conversations::ConversationListRow>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // ?kind=plan selects plans; every other value (absent included) → chats.
    let kind = uri
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(key, _)| key == "kind")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    let kind = if kind == "plan" { "plan" } else { "chat" };
    // ?archived=1 — the exact string '1' — asks for the retired rows;
    // everything else sees the live ones. Same spelling as /api/boards.
    let archived = uri
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(key, _)| key == "archived")
                .map(|(_, v)| v.into_owned())
        })
        .as_deref()
        == Some("1");
    Ok(
        match list_conversations(&state.pg, &user.id, kind, archived).await {
            Ok(rows) => (
                StatusCode::OK,
                Json(ConversationsEnvelope {
                    conversations: rows,
                }),
            )
                .into_response(),
            Err(e) => internal("[conversations] list failed", e),
        },
    )
}

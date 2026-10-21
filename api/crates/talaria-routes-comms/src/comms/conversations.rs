// /api/conversations. GET
// ?kind=plan → the user's plan conversations; ?kind=work → their work
// sessions; anything else → their chats.
// ?archived=1 → the retired set (exact string '1'); everything else is live.
// Newest activity first; the client groups them by agent.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use talaria_conversations::list_conversations_in;
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
    // A WHITELIST, not a pass-through: `kind` reaches a SQL bind, and an
    // unknown one would silently list nothing rather than the chats the caller
    // asked for by omission.
    //
    // `research` IS a kind and was missing from this list, so asking for it
    // returned CHATS — a quiet wrong answer rather than an empty one, which is
    // the worse of the two failures and the harder to notice.
    //
    // `sessions` is the merged surface's own question: the kinds that are a
    // session you work in, as one list. Named rather than a comma-separated
    // pass-through, so the server owns what is in the set — a caller that
    // could compose arbitrary combinations could ask for one the audience
    // predicate was never written for.
    let kind = uri
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(key, _)| key == "kind")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    let kinds: &[&str] = match kind.as_str() {
        "plan" => &["plan"],
        "work" => &["work"],
        "research" => &["research"],
        "sessions" => &["work", "plan", "research"],
        _ => &["chat"],
    };
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
        match list_conversations_in(&state.pg, &user.id, kinds, archived).await {
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

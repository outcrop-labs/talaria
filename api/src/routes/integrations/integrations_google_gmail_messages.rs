// GET /api/integrations/google/gmail/messages?q= — port of
// ui/src/routes/api/integrations/google.gmail.messages.ts. Recent mail
// (metadata only).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::google::errors::google_fail;
use crate::google::gmail::list_recent_messages;
use crate::google::oauth::query_pairs;
use crate::session::require_user;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // `|| 'in:inbox'` — an absent OR EMPTY q both fold to the inbox default.
    let q = query_pairs(uri.query())
        .get("q")
        .cloned()
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| "in:inbox".to_string());
    let sb = state.secretbox().await.unwrap_or_default();
    match list_recent_messages(&state.pg, &sb, &user.id, now_ms(), 8, &q).await {
        Ok(messages) => Json(json!({ "messages": messages })).into_response(),
        Err(e) => google_fail(e, "Gmail"),
    }
}

/// Date.now() — the one clock the mailbox read carries.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

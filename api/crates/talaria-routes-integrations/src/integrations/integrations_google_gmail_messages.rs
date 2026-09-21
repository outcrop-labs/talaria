// GET /api/integrations/google/gmail/messages?q= — recent mail (metadata
// only).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::now_ms;
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::gmail::list_recent_messages;
use talaria_api_facades::google::oauth::query_pairs;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // An absent OR EMPTY q both fold to the inbox default.
    let q = query_pairs(uri.query())
        .get("q")
        .cloned()
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| "in:inbox".to_string());
    let sb = state.secretbox().await.unwrap_or_default();
    Ok(
        match list_recent_messages(&state.pg, &sb, &user.id, now_ms(), 8, &q).await {
            Ok(messages) => Json(json!({ "messages": messages })).into_response(),
            Err(e) => google_fail(e, "Gmail"),
        },
    )
}

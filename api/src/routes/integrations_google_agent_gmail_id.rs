// GET /api/integrations/google/agent/gmail/{id} — port of
// ui/src/routes/api/integrations/google.agent.gmail.$id.ts. One FULL message
// (headers + plain-text body) for the calling agent. The listing tool hands
// out ids and snippets; this is the read an actual answer needs. Reads are
// free (confirm-sends govern the outbound half only).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::{AgentSubject, refuse_legacy, require_agent};
use crate::error::{house_error, house_error_msg};
use crate::gmail::get_message_with_token;
use crate::google_agent::resolve_agent_google;
use crate::google_errors::google_fail;
use crate::state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // Acting as a HUMAN — the owner's mailbox (or the shared org one). A
    // legacy shared-key caller only ASSERTS which agent it is, so it never
    // reaches the token.
    if let Some(denied) = refuse_legacy(&caller, "Gmail access") {
        return denied;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "No Google account is connected for this agent (its owner, or the org account).",
        );
    };
    // Hand-checked (not a path param constraint) so the shape of the refusal
    // stays this route's own 400, not the router's.
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || !(4..=64).contains(&id.len())
    {
        return house_error(StatusCode::BAD_REQUEST, "bad request");
    }
    match get_message_with_token(&google.token, &id).await {
        Ok(message) => Json(json!({ "message": message })).into_response(),
        Err(e) => google_fail(e, "Gmail"),
    }
}

/// Date.now() — the one clock the agent mailbox reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

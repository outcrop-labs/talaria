// /api/integrations/google/agent/gmail/labels — port of
// ui/src/routes/api/integrations/google.agent.gmail.labels.ts. The label half
// of inbox organizing. Gmail's folders ARE labels: INBOX and UNREAD are system
// labels a message carries, and "filing" mail means applying and removing them
// (see the organize route for the mutations).
// GET  → every label (read)
// POST → find-or-create a label (safe to retry)

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::{AgentSubject, refuse_legacy, require_agent};
use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, house_error_msg};
use crate::google::agent::resolve_agent_google;
use crate::google::errors::google_fail;
use crate::google::gmail::{create_label_with_token, list_labels_with_token};
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // The label list sketches the mailbox's whole filing scheme — a legacy
    // shared-key caller only ASSERTS which agent it is, so it never sees one.
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
    match list_labels_with_token(&google.token).await {
        Ok(labels) => Json(json!({ "labels": labels })).into_response(),
        Err(e) => google_fail(e, "Gmail"),
    }
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // Creating a label rewrites the mailbox's filing scheme — same proof bar.
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
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 60) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match create_label_with_token(&google.token, &name).await {
        Ok(label) => Json(json!({ "label": label })).into_response(),
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

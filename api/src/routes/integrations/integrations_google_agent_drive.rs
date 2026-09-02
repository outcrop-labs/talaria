// GET /api/integrations/google/agent/drive?q= — find files in the Drive the
// calling agent acts for (its owner's, or the shared org Drive).
// Read-only: finding and handing back a link. Creating files stays on
// export_to_google_doc; nothing here writes.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::{AgentSubject, refuse_legacy, require_agent};
use crate::error::house_error_msg;
use crate::google::agent::resolve_agent_google;
use crate::google::drive::list_drive_files_with_token;
use crate::google::errors::google_fail;
use crate::google::oauth::query_pairs;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // A Drive listing is the owner's (or the org's) file inventory — a legacy
    // shared-key caller only ASSERTS which agent it is, so it never sees one.
    if let Some(denied) = refuse_legacy(&caller, "Drive access") {
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
    // Only an ABSENT q means no filter; an empty one rides in (and the
    // engine's trim makes it no filter either).
    let q = query_pairs(uri.query()).get("q").cloned();
    match list_drive_files_with_token(&google.token, q.as_deref(), 25).await {
        Ok(files) => Json(json!({ "files": files })).into_response(),
        Err(e) => google_fail(e, "Drive"),
    }
}

/// Epoch-ms clock — the one time the agent Drive surface reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

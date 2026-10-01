// POST /api/workbench/auth/v1/credential/{id}/disable. The harness telling us
// a credential is dead.
//
// omp calls this when a provider answers in a way that means "this grant will
// never work again". We keep the row — with the cause, so the UI can say what
// happened — and stop serving it in snapshots. Only a re-login in the UI
// brings it back, which is correct: there is nothing an agent could do to
// revive it.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_body::{as_object, parse, string_member};
use talaria_coding_accounts as ca;
use talaria_error::internal;
use talaria_state::AppState;

use super::coding_broker::{broker_agent, owned_account};

// doc: Mark one of the calling agent's coding-account credentials dead. The
// doc: row and its cause are kept for the UI; snapshots stop serving it.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Response, Response> {
    let agent_id = broker_agent(&state, &headers).await?;
    owned_account(&state, &agent_id, id).await?;
    // The client always sends a cause; a missing one is still a disable, with
    // the honest placeholder rather than a 400 that leaves a dead credential
    // being served.
    let cause = as_object(&parse(&body))
        .ok()
        .and_then(|o| string_member(o, "cause", 1, 500).ok())
        .unwrap_or_else(|| "the harness reported this credential as unusable".to_string());
    ca::disable_account(&state.pg, id, &cause)
        .await
        .map_err(|e| internal("[workbench/auth] disable failed", e))?;
    Ok(axum::Json(json!({ "ok": true })).into_response())
}

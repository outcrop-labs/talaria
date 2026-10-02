// DELETE /api/workbench/auth/v1/credential/{id}/blocks. Forget every
// remembered rate-limit block for one of the calling agent's coding accounts
// — the client's "start clean" after a credential comes back to life.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_coding_accounts as ca;
use talaria_error::internal;
use talaria_state::AppState;

use super::coding_broker::{broker_agent, owned_account};

// doc: Forget every remembered rate-limit block for this coding account.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, Response> {
    let agent_id = broker_agent(&state, &headers).await?;
    owned_account(&state, &agent_id, id).await?;
    ca::clear_blocks(&state.pg, id, None)
        .await
        .map_err(|e| internal("[workbench/auth] blocks clear failed", e))?;
    Ok(axum::Json(json!({ "ok": true })).into_response())
}

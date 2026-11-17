// /api/integrations/google. This user's Google connection: status (never
// exposes tokens) and disconnect (forget, and revoke unless the grant is
// shared with another stored connection).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::google::connections::{disconnect, get_connection_status, revoke_note};
use talaria_api_facades::google::oauth::google_integration_enabled;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

// GET → this user's Google connection status (never exposes tokens)
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    let available = google_integration_enabled(&state.pg, &sb).await;
    let status = match get_connection_status(&state.pg, &user.id).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[integrations/google] status read failed", e)),
    };
    Ok(
        // wire key order: available, then the status fields.
        Json(json!({
            "available": available,
            "connected": status.connected,
            "email": status.email,
            "scope": status.scope,
            "connectedAt": status.connected_at,
        }))
        .into_response(),
    )
}

// DELETE → disconnect: forget the connection, and revoke its grant at
// Google unless another stored connection (the org account, an agent
// identity) uses the same Google account — revoking would cut that one off
// too. `revoked` says whether Google accepted a revoke; `revokeNote` is the
// sentence when it did not (Google's own words when Google refused), null
// otherwise.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    let outcome = disconnect(&state.pg, &sb, &user.id).await;
    Ok(Json(json!({
        "ok": true,
        "revoked": outcome.revoked(),
        "revokeNote": revoke_note(&outcome, Some(user.id.as_str())),
    }))
    .into_response())
}

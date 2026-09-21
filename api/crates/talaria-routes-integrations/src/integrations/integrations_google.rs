// /api/integrations/google. This user's Google connection: status (never
// exposes tokens) and disconnect (revoke + forget).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::google::connections::{disconnect, get_connection_status};
use talaria_api_facades::google::oauth::google_integration_enabled;
use talaria_error::thrown_internal_error;
use talaria_session::require_user;
use talaria_state::AppState;

// GET → this user's Google connection status (never exposes tokens)
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let available = google_integration_enabled(&state.pg, &sb).await;
    let status = match get_connection_status(&state.pg, &user.id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[integrations/google] status read failed: {e}");
            return thrown_internal_error();
        }
    };
    // wire key order: available, then the status fields.
    Json(json!({
        "available": available,
        "connected": status.connected,
        "email": status.email,
        "scope": status.scope,
        "connectedAt": status.connected_at,
    }))
    .into_response()
}

// DELETE → disconnect (revoke + forget)
pub async fn delete(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = state.secretbox().await.unwrap_or_default();
    disconnect(&state.pg, &sb, &user.id).await;
    Json(json!({ "ok": true })).into_response()
}

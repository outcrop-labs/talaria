// GET /api/integrations/google/drive/files?q= — browse/search the user's
// Drive.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::google::drive::list_drive_files;
use crate::google::errors::google_fail_with;
use crate::google::oauth::query_pairs;
use crate::session::require_user;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // An EMPTY q is a query (the engine's trim decides it is no filter); only
    // an absent one is None.
    let q = query_pairs(uri.query()).get("q").cloned();
    let sb = state.secretbox().await.unwrap_or_default();
    match list_drive_files(&state.pg, &sb, &user.id, now_ms(), q.as_deref(), 25).await {
        Ok(files) => Json(json!({ "files": files })).into_response(),
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

/// Epoch-ms clock — the one time the Drive surface reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

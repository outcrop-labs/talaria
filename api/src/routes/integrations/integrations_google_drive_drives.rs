// /api/integrations/google/drive/drives. The Drive roster: every Drive this
// person can browse across BOTH connections — personal My Drive, shared
// drives their own account joined, the org connection's Shared Drive and My
// Drive. NotConnected (409) only when BOTH connections are absent; a missing
// one simply omits its entries.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::google::drive::drive_roster;
use crate::google::errors::{GoogleError, google_fail_with};
use crate::session::require_user;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = state.secretbox().await.unwrap_or_default();
    match drive_roster(&state.pg, &sb, &user.id, now_ms()).await {
        Ok(drives) => {
            if drives.is_empty() {
                // Both connections absent — the connect screen's answer.
                google_fail_with(
                    GoogleError::NotConnected,
                    "Drive",
                    "Connect a Google account to browse its Drive.",
                )
            } else {
                Json(json!({ "drives": drives })).into_response()
            }
        }
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

/// Epoch-ms clock — the one time the Drive surface reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

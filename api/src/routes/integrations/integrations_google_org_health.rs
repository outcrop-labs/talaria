// GET /api/integrations/google/org/health — live probe of Drive / Calendar /
// Gmail with the org connection's token. Admin-only, and
// deliberately a separate route from the org status read: it makes three real
// Google calls and must only run when an admin asks for it, not on every
// panel load.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::epoch_ms_to_iso;
use crate::google::api_health::probe_org_google_apis;
use crate::google::errors::google_fail;
use crate::session::require_admin;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    // checkedAt is stamped BEFORE the await, so the timestamp names when the
    // probe STARTED, not when it answered.
    let checked_at = epoch_ms_to_iso(now_ms());
    match probe_org_google_apis(&state.pg, &sb, now_ms()).await {
        Ok(results) => Json(json!({
            "checkedAt": checked_at,
            "results": results,
        }))
        .into_response(),
        Err(e) => google_fail(e, "APIs"),
    }
}

/// Epoch-ms clock — the one time the probe reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

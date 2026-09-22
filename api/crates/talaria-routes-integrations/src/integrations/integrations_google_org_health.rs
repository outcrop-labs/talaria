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

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_agent_auth::now_ms;
use talaria_api_facades::google::api_health::probe_org_google_apis;
use talaria_api_facades::google::errors::google_fail;
use talaria_session::require_admin;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    // checkedAt is stamped BEFORE the await, so the timestamp names when the
    // probe STARTED, not when it answered.
    let checked_at = epoch_ms_to_iso(now_ms());
    Ok(
        match probe_org_google_apis(&state.pg, &sb, now_ms()).await {
            Ok(results) => Json(json!({
                "checkedAt": checked_at,
                "results": results,
            }))
            .into_response(),
            Err(e) => google_fail(e, "APIs"),
        },
    )
}

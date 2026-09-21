// /api/artifacts/for.
// Artifacts attached to a given target (e.g. a KB doc), filtered to the ones
// the caller can read. GET /api/artifacts/for?targetType=kb-doc&targetId=<id>
// — a static path that must win over /api/artifacts/{id} in the router.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::collections::HashMap;

use talaria_api_facades::kb::perms::can_read;
use talaria_artifacts::{artifacts_for_target, guarded};
use talaria_error::internal;
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let (Some(target_type), Some(target_id)) = (q.get("targetType"), q.get("targetId")) else {
        return Json(json!({ "artifacts": [] })).into_response();
    };
    if target_type.is_empty() || target_id.is_empty() {
        return Json(json!({ "artifacts": [] })).into_response();
    }
    let artifacts = match artifacts_for_target(&state.pg, target_type, target_id).await {
        Ok(a) => a,
        Err(e) => return internal("[artifacts] for-target read failed", e),
    };
    let who = who_of(&user);
    let artifacts: Vec<_> = artifacts
        .iter()
        .filter(|a| can_read(&guarded(a), Some(&user.id), who.as_deref(), &[], &[]))
        .collect();
    Json(json!({ "artifacts": artifacts })).into_response()
}

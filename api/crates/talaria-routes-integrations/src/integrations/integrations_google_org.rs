// /api/integrations/google/org — the shared org Google connection
// (admin-managed); general fleet agents act as this identity for
// Drive/Docs/Calendar/Gmail. GET → status + targets · PUT → save build
// targets · DELETE → disconnect.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::google::oauth::google_integration_enabled;
use talaria_api_facades::google::org::{
    OrgTargetsPatch, disconnect_org, get_org_connection_status, set_org_targets,
};
use talaria_body::{nullish_max_string_member, nullish_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_admin;
use talaria_state::AppState;

// GET → status + targets
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    let available = google_integration_enabled(&state.pg, &sb).await;
    let status = match get_org_connection_status(&state.pg).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[integrations/google/org] status read failed", e)),
    };
    Ok(
        // Wire key order: available, then the status fields, then targets (whose
        // own key order is TargetsWire's declaration order).
        Json(json!({
            "available": available,
            "connected": status.connected,
            "email": status.email,
            "scope": status.scope,
            "connectedAt": status.connected_at,
            "targets": status.targets,
        }))
        .into_response(),
    )
}

// PUT → save build targets (absent keys leave the column alone; null clears)
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // The PATCH tri-state: absent = leave the column alone, null = clear it,
    // a string = set it (blank clears too, in the engine's norm()).
    let patch = OrgTargetsPatch {
        drive_folder_id: match read_target(obj, "driveFolderId", 200) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
        calendar_id: match read_target(obj, "calendarId", 300) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
        send_as: match read_target(obj, "sendAs", 300) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    if let Err(e) = set_org_targets(&state.pg, &patch).await {
        return Ok(internal(
            "[integrations/google/org] targets write failed",
            e,
        ));
    }
    let status = match get_org_connection_status(&state.pg).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[integrations/google/org] status read failed", e)),
    };
    Ok(Json(json!({
        "ok": true,
        "connected": status.connected,
        "email": status.email,
        "scope": status.scope,
        "connectedAt": status.connected_at,
        "targets": status.targets,
    }))
    .into_response())
}

/// One nullish target column, as the tri-state the engine's patch wants.
fn read_target(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    max: usize,
) -> Result<Option<Option<String>>, String> {
    nullish_member(obj, key, |o, k| nullish_max_string_member(o, k, max))
}

// DELETE → disconnect the org connection
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    disconnect_org(&state.pg, &sb).await;
    Ok(Json(json!({ "ok": true })).into_response())
}

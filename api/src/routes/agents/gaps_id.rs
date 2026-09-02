// /api/gaps/{id}. PUT status (open | dismissed | resolved) — agents.manage. Dismissed shapes
// that keep recurring reopen automatically; resolved sticks.

use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::gaps::set_gap_status;
use crate::session::require_perm;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // status: exactly one of the three literals — anything else is a 400
    // before the row is touched.
    let status = match string_member(obj, "status", 1, 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if !matches!(status.as_str(), "open" | "dismissed" | "resolved") {
        return house_error(
            StatusCode::BAD_REQUEST,
            "status must be one of open, dismissed, resolved",
        );
    }
    match set_gap_status(&state.pg, &id, &status).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => {
            tracing::error!("[gaps] status set failed: {e}");
            thrown_internal_error()
        }
    }
}

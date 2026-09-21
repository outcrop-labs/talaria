// /api/gaps/{id}. PUT status (open | dismissed | resolved) — agents.manage. Dismissed shapes
// that keep recurring reopen automatically; resolved sticks.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_gaps::set_gap_status;
use talaria_session::require_perm;
use talaria_state::AppState;

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // status: exactly one of the three literals — anything else is a 400
    // before the row is touched.
    let status = match string_member(obj, "status", 1, 20) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if !matches!(status.as_str(), "open" | "dismissed" | "resolved") {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "status must be one of open, dismissed, resolved",
        ));
    }
    Ok(match set_gap_status(&state.pg, &id, &status).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => internal("[gaps] status set failed", e),
    })
}

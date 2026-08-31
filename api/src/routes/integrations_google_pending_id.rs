// POST /api/integrations/google/pending/{id} — port of
// ui/src/routes/api/integrations/google.pending.$id.ts. Approve (executes as
// the owner, or the org for org actions) or reject an agent-drafted action.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, enum_member, parse};
use crate::error::{house_error, house_error_msg};
use crate::google_pending_actions::decide_action;
use crate::session::require_user;
use crate::state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let decision = match enum_member(obj, "decision", &["approve", "reject"]) {
        Ok(d) => d,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let sb = state.secretbox().await.unwrap_or_default();
    let outcome = match decide_action(
        &state.pg,
        &sb,
        &id,
        &user.id,
        user.role == "admin",
        &decision,
        now_ms(),
    )
    .await
    {
        Ok(o) => o,
        // Every path TS lets THROW (a DB error, a token read that blew up
        // rather than answered null) — the catch's 500.
        Err(e) => {
            tracing::error!("[integrations/google/pending] decide failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(outcome) = outcome else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    match outcome.status.as_str() {
        "forbidden" => house_error(StatusCode::FORBIDDEN, "forbidden"),
        "not_connected" => house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            outcome.message.as_deref().unwrap_or_default(),
        ),
        "failed" => house_error_msg(
            StatusCode::BAD_GATEWAY,
            "failed",
            outcome.message.as_deref().unwrap_or_default(),
        ),
        status => Json(json!({ "status": status })).into_response(),
    }
}

/// Date.now() — the one clock a decision executes under.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

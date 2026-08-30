// /api/channels/{id}/conclude — port of ui/src/routes/api/channels.$id.conclude.ts.
// POST → conclude a Relay: summarize what was decided (posted as the final
// message + indexed for retrieval), then archive it. Members only; relays
// only — channels persist. The summarize failures surface as 502 with
// concludeRelay's user-facing copy as the body.

use crate::channels::channel_role;
use crate::comms_decay::conclude_relay;
use crate::error::house_error;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !member(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let row: Option<(String, String)> = match sqlx::query_as(
        "select name, kind from channels where id = $1::uuid and archived_at is null",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[channels] conclude read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some((name, kind)) = row else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    if kind != "group" {
        return house_error(
            StatusCode::BAD_REQUEST,
            "only relays conclude — channels persist",
        );
    }
    match conclude_relay(&state, &id, &user.id, &name).await {
        // The catch arm's shape: the message IS the copy, the status is 502.
        Ok(summary) => Json(json!({ "summary": summary })).into_response(),
        Err(e) => house_error(StatusCode::BAD_GATEWAY, &e),
    }
}

async fn member(state: &AppState, user_id: &str, id: &str) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("[channels] role read on conclude failed: {e}");
            false
        }
    }
}

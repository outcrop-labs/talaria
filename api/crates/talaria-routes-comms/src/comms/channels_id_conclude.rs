// /api/channels/{id}/conclude.
// POST → conclude a Relay: summarize what was decided (posted as the final
// message + indexed for retrieval), then archive it. Members only; relays
// only — channels persist. The summarize failures surface as 502 with
// conclude_relay's user-facing copy as the body.

use super::{ChannelNeed, channel_gate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_comms_decay::conclude_relay;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !channel_gate(&state, &user.id, &id, ChannelNeed::Member, " on conclude").await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let row: Option<(String, String)> = match sqlx::query_as(
        "select name, kind from channels where id = $1::uuid and archived_at is null",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[channels] conclude read failed", e)),
    };
    let Some((name, kind)) = row else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    if kind != "group" {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "only relays conclude — channels persist",
        ));
    }
    Ok(match conclude_relay(&state, &id, &user.id, &name).await {
        // On Err the message IS the user-facing copy; the status is 502.
        Ok(summary) => Json(json!({ "summary": summary })).into_response(),
        Err(e) => house_error(StatusCode::BAD_GATEWAY, &e),
    })
}

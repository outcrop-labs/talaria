// /api/dms. POST { userId } → find-or-create the DM with that person (rides
// the channel machinery: same messages, SSE feed, and composer as
// everything else).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_body::uuid_member;
use talaria_channels::ensure_dm;
use talaria_error::{house_error, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct DmEnvelope {
    channel: talaria_channels::CreatedChannel,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let other = match uuid_member(obj, "userId") {
        Ok(u) => u,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        // ensure_dm's own user-facing message (DM with yourself, unknown user)
        // answers as a 400.
        match ensure_dm(&state.pg, &user.id, &other).await {
            Ok(channel) => (StatusCode::OK, Json(DmEnvelope { channel })).into_response(),
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        },
    )
}

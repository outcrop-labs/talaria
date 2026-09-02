// /api/dms — port of ui/src/routes/api/dms.ts. POST { userId } → find-or-
// create the DM with that person (rides the channel machinery: same messages,
// SSE feed, and composer as everything else).

use crate::body::{as_object, uuid_member};
use crate::channels::ensure_dm;
use crate::error::house_error;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct DmEnvelope {
    channel: crate::channels::CreatedChannel,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let other = match uuid_member(obj, "userId") {
        Ok(u) => u,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // ensureDm throws its user-facing message (DM with yourself, unknown
    // user); the TS route answers it as a 400.
    match ensure_dm(&state.pg, &user.id, &other).await {
        Ok(channel) => (StatusCode::OK, Json(DmEnvelope { channel })).into_response(),
        Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
    }
}

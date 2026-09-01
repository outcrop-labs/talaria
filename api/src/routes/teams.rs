// /api/teams — port of ui/src/routes/api/teams.ts. GET → the caller's teams,
// resolved through ACTING user (a personal assistant acts as its owner — the
// identity-proxy model); POST { name } → create (humans only: requireUser).

use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{acting_user, require_user, unauthorized};
use crate::state::AppState;
use crate::teams::{create_team, list_teams};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    };
    match list_teams(&state.pg, &user.id).await {
        Ok(teams) => Json(json!({ "teams": teams })).into_response(),
        Err(e) => {
            tracing::error!("[teams] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
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
    let name = match string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (id, team_name, created_ms) = match create_team(&state.pg, &user.id, &name).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[teams] create failed: {e}");
            return thrown_internal_error();
        }
    };
    // TS spreads {id, name, createdAt} then role then memberCount — the
    // create response's key order differs from the list's on purpose.
    Json(json!({
        "team": {
            "id": id,
            "name": team_name,
            "createdAt": crate::agent_auth::epoch_ms_to_iso(created_ms),
            "role": "owner",
            "memberCount": 1
        }
    }))
    .into_response()
}

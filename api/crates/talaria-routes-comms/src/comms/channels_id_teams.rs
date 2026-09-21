// /api/channels/{id}/teams.
// POST { teamId } → grant a team (any member). DELETE { teamId } → revoke
// (owner, or any member removing a team they belong to). Direct messages
// stay private.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{as_object, uuid_member};
use talaria_channels::{add_channel_team, channel_role, remove_channel_team};
use talaria_error::{house_error, thrown_internal_error};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_teams::{get_team, team_role};

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !member(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = talaria_body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match get_team(&state.pg, &team_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::BAD_REQUEST, "team not found"),
        Err(e) => {
            tracing::error!("[channels] team lookup on grant failed: {e}");
            return thrown_internal_error();
        }
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    match add_channel_team(&notify, &id, &team_id).await {
        Ok(None) => Json(json!({ "ok": true })).into_response(),
        Ok(Some(error)) => house_error(StatusCode::BAD_REQUEST, &error),
        Err(e) => {
            tracing::error!("[channels] team grant failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[channels] role read on team revoke failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(role) = role else {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    };
    let parsed = talaria_body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if role != "owner" {
        match team_role(&state.pg, &user.id, &team_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
            Err(e) => {
                tracing::error!("[channels] team role read on revoke failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = remove_channel_team(&notify, &id, &team_id).await {
        tracing::error!("[channels] team revoke failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

async fn member(state: &AppState, user_id: &str, id: &str) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("[channels] role read on teams failed: {e}");
            false
        }
    }
}

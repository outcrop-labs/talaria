// /api/channels/{id}/teams.
// POST { teamId } → grant a team (any member). DELETE { teamId } → revoke
// (owner, or any member removing a team they belong to). Direct messages
// stay private.

use super::{ChannelNeed, channel_gate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::uuid_member;
use talaria_channels::{add_channel_team, channel_role, remove_channel_team};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_teams::{get_team, team_role};

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !channel_gate(&state, &user.id, &id, ChannelNeed::Member, " on teams").await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match get_team(&state.pg, &team_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::BAD_REQUEST, "team not found")),
        Err(e) => return Ok(internal("[channels] team lookup on grant failed", e)),
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    Ok(match add_channel_team(&notify, &id, &team_id).await {
        Ok(None) => Json(json!({ "ok": true })).into_response(),
        Ok(Some(error)) => house_error(StatusCode::BAD_REQUEST, &error),
        Err(e) => internal("[channels] team grant failed", e),
    })
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[channels] role read on team revoke failed", e)),
    };
    let Some(role) = role else {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    };
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if role != "owner" {
        match team_role(&state.pg, &user.id, &team_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
            Err(e) => return Ok(internal("[channels] team role read on revoke failed", e)),
        }
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = remove_channel_team(&notify, &id, &team_id).await {
        return Ok(internal("[channels] team revoke failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

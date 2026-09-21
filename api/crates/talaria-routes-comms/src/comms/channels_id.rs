// /api/channels/{id}.
// GET → channel detail (role + members + agents + teams). PUT → rename / set topic
// (owner). DELETE → archive (?hard=1 deletes; owner only; a hard delete also
// purges the channel's activity points so nothing orphans in the index).

use super::{ChannelNeed, channel_gate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::retrieval::qdrant;
use talaria_api_facades::retrieval::sources::{ActivityField, purge_activity_by_field};
use talaria_body::{optional_string_member, present_nullable_max_string_member};
use talaria_channels::{
    archive_channel, channel_role, delete_channel, is_task_room, list_channel_agents,
    list_channel_members, list_channel_teams, list_task_room_agents, list_task_room_members,
    update_channel,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[channels] role read on GET failed", e)),
    };
    let Some(role) = role else {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    };
    // A task room's roster is its board's audience and the board's agent
    // policy — read, not provisioned — while an ordinary channel serves the
    // rows it owns. Same wire shape either way: the room embed renders
    // exactly what a rail channel does.
    let task_room = match is_task_room(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[channels] task-room probe failed", e)),
    };
    let (members, agents) = if task_room {
        let m = match list_task_room_members(&state.pg, &id).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[channels] task-room member read failed", e)),
        };
        let a = match list_task_room_agents(&state.pg, &id).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[channels] task-room agent read failed", e)),
        };
        (m, a)
    } else {
        let m = match list_channel_members(&state.pg, &id).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[channels] member read failed", e)),
        };
        let a = match list_channel_agents(&state.pg, &id).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[channels] agent read failed", e)),
        };
        (m, a)
    };
    let teams = match list_channel_teams(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[channels] team read failed", e)),
    };
    Ok(
        Json(json!({ "role": role, "members": members, "agents": agents, "teams": teams }))
            .into_response(),
    )
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // The owner check stands before the body parse.
    if !channel_gate(&state, &user.id, &id, ChannelNeed::Owner, "").await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    // name: min 1, max 80, optional — the empty name is a min failure, not
    // a value (unlike the topic below).
    let name = match optional_string_member(obj, "name", 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Three states, not two (max 300): absent leaves the topic alone,
    // present-null clears it, a string sets it.
    let topic = match present_nullable_max_string_member(obj, "topic", 300) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = update_channel(
        &notify,
        &id,
        name.as_deref(),
        topic.as_ref().map(|t| t.as_deref()),
    )
    .await
    {
        return Ok(internal("[channels] update failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: axum::http::Uri,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !channel_gate(&state, &user.id, &id, ChannelNeed::Owner, "").await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    // ?hard=1 — exact match — hard-deletes; any other value archives.
    let hard = uri
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "hard")
                .map(|(_, v)| v.into_owned())
        })
        .as_deref()
        == Some("1");
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let result = if hard {
        delete_channel(&notify, &id).await
    } else {
        archive_channel(&notify, &id).await
    };
    if let Err(e) = result {
        return Ok(internal("[channels] archive/delete failed", e));
    }
    // A hard delete removes the channel's messages — purge their activity
    // points too so nothing orphans in the index. Fire-and-forget: the
    // purge's errors are swallowed.
    if hard {
        let pg = state.pg.clone();
        let channel_id = id.clone();
        tokio::spawn(async move {
            let qd = qdrant::real_deps();
            let _ = purge_activity_by_field(&pg, &qd, ActivityField::ChannelId, &channel_id).await;
        });
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

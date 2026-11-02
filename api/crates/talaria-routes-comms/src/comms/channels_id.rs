// /api/channels/{id}.
// GET → channel detail (role + members + agents + teams + agentInitiative +
// the caller's own notify level and mute).
// PUT → rename / set topic / set agentInitiative (owner). DELETE → archive (?hard=1 deletes; owner only; a hard delete also
// purges the channel's activity points so nothing orphans in the index).

use super::{ChannelNeed, channel_gate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_api_facades::retrieval::qdrant;
use talaria_api_facades::retrieval::sources::{ActivityField, purge_activity_by_field};
use talaria_body::{optional_string_member, present_nullable_max_string_member};
use talaria_channels::{
    archive_channel, channel_role, delete_channel, is_task_room, list_channel_agents,
    list_channel_members, list_channel_teams, list_task_room_agents, list_task_room_members,
    update_channel,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{ChannelLevel, NotifyDeps};
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
    // WHETHER AN AGENT HERE MAY SPEAK UNPROMPTED. Served whatever the
    // workspace-wide switch says, because this is the room's own answer and a
    // person setting it should not have to know whether the capability is on
    // yet — the two gates are independent and both have to be open.
    let agent_initiative: bool =
        sqlx::query_scalar("select agent_initiative from channels where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
    let notify = match caller_notify(&state.pg, &id, &user.id).await {
        Ok(n) => n,
        Err(e) => return Ok(internal("[channels] notify level read failed", e)),
    };
    Ok(Json(json!({
        "role": role,
        "members": members,
        "agents": agents,
        "teams": teams,
        "agentInitiative": agent_initiative,
        "notify": notify,
    }))
    .into_response())
}

/// The caller's own notify level and mute for this channel, as `PUT
/// …/notify` sets them: `{level, mutedUntil, settable}`. `level` is what
/// applies — a NULL stored level is the kind's default (a DM `all`, a
/// channel `mentions`, O-14). `mutedUntil` is null once a mute has run out.
/// `settable` is false when the caller has no member row of their own to
/// hold a level (a task room's roster is its board's; a team grant is the
/// team's) — the PUT answers 409 there, and a client hides the control.
async fn caller_notify(
    pg: &sqlx::PgPool,
    channel_id: &str,
    user_id: &str,
) -> Result<serde_json::Value, sqlx::Error> {
    let row: Option<(String, bool, Option<String>, Option<i64>)> = sqlx::query_as(
        "select c.kind, cm.user_id is not null, cm.notify_level, \
                case when cm.muted_until > now() \
                     then (trunc(extract(epoch from cm.muted_until) * 1000))::bigint end \
         from channels c \
         left join channel_members cm on cm.channel_id = c.id and cm.user_id = $2::uuid \
         where c.id = $1::uuid",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let (kind, settable, level, muted_ms) =
        row.unwrap_or_else(|| ("channel".into(), false, None, None));
    Ok(json!({
        "level": ChannelLevel::of(level.as_deref(), &kind).as_str(),
        "mutedUntil": muted_ms.map(epoch_ms_to_iso),
        "settable": settable,
    }))
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
    // Its own write rather than a field on `update_channel`: renaming a room
    // and changing whether agents may speak in it are different decisions, and
    // a rename should not be able to carry the second one by accident.
    if obj.contains_key("agentInitiative") {
        let on = match talaria_body::boolean_member(obj, "agentInitiative") {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        if let Err(e) = sqlx::query("update channels set agent_initiative = $2 where id = $1::uuid")
            .bind(&id)
            .bind(on)
            .execute(&state.pg)
            .await
        {
            return Ok(internal("[channels] agent-initiative write failed", e));
        }
    }
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

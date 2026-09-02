// /api/channels/{id}.
// GET → channel detail (role + members + agents). PUT → rename / set topic
// (owner). DELETE → archive (?hard=1 deletes; owner only; a hard delete also
// purges the channel's activity points so nothing orphans in the index).

use crate::body::{as_object, optional_string_member, present_nullable_max_string_member};
use crate::channels::{
    archive_channel, channel_role, delete_channel, list_channel_agents, list_channel_members,
    update_channel,
};
use crate::error::{house_error, thrown_internal_error};
use crate::notify::NotifyDeps;
use crate::retrieval::qdrant;
use crate::retrieval::sources::{ActivityField, purge_activity_by_field};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[channels] role read on GET failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(role) = role else {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    };
    let members = match list_channel_members(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] member read failed: {e}");
            return thrown_internal_error();
        }
    };
    let agents = match list_channel_agents(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] agent read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "role": role, "members": members, "agents": agents })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // The owner check stands before the body parse.
    if !owner_gate(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // name: min 1, max 80, optional — the empty name is a min failure, not
    // a value (unlike the topic below).
    let name = match optional_string_member(obj, "name", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Three states, not two (max 300): absent leaves the topic alone,
    // present-null clears it, a string sets it.
    let topic = match present_nullable_max_string_member(obj, "topic", 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
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
        tracing::error!("[channels] update failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: axum::http::Uri,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !owner_gate(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
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
        tracing::error!("[channels] archive/delete failed: {e}");
        return thrown_internal_error();
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
    Json(json!({ "ok": true })).into_response()
}

/// The PUT/DELETE gate: role must read exactly 'owner'.
async fn owner_gate(state: &AppState, user_id: &str, id: &str) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(Some(role)) => role == "owner",
        Ok(None) => false,
        Err(e) => {
            tracing::error!("[channels] role read failed: {e}");
            false
        }
    }
}

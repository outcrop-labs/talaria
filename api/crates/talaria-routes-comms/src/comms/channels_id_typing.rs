// /api/channels/{id}/typing.
// POST { typing?: boolean, threadRootId?: uuid } → tell the channel's other
// members that the caller is typing (default) or has stopped — in the channel
// itself, or in one thread when `threadRootId` is set. Ephemeral: nothing is
// stored. The signal goes over the channel's own stream (the open
// conversation's or thread panel's "Maya is typing…"). Channel-level typing
// also goes to every other member's own stream, which is what their Comms rail
// listens to for the dots beside a row; thread typing stays in the thread.
// Composers send `typing: true` at most every few seconds while text changes
// and `false` on send or clear; listeners expire a silent `true` themselves.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{optional_boolean_member, uuid_member};
use talaria_channels::{channel_role, list_channel_members};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_realtime_watch::{UserEvent, publish_typing, publish_user};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[channels] role read on typing failed", e)),
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let typing = match optional_boolean_member(obj, "typing") {
        Ok(v) => v.unwrap_or(true),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let thread_root_id = if obj.contains_key("threadRootId") {
        match uuid_member(obj, "threadRootId") {
            Ok(v) => Some(v),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    publish_typing(
        &notify.realtime,
        &id,
        &user.id,
        thread_root_id.as_deref(),
        typing,
    );
    if thread_root_id.is_some() {
        return Ok(Json(json!({ "ok": true })).into_response());
    }

    let kind: Option<String> =
        match sqlx::query_scalar("select kind from channels where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(k) => k,
            Err(e) => return Ok(internal("[channels] kind read on typing failed", e)),
        };
    // The rail's rows: DMs, channels and relays (not task rooms, which live on
    // their tickets rather than in the rail).
    if matches!(kind.as_deref(), Some("dm" | "channel" | "group")) {
        let members = match list_channel_members(&state.pg, &id).await {
            Ok(m) => m,
            Err(e) => return Ok(internal("[channels] member read on typing failed", e)),
        };
        for m in members.iter().filter(|m| m.user_id != user.id) {
            publish_user(
                &notify.realtime,
                &m.user_id,
                &UserEvent::Typing {
                    channel_id: id.clone(),
                    user_id: user.id.clone(),
                    typing,
                },
            );
        }
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

// /api/channels/{id}/notify.
// PUT { level?: "all"|"mentions"|"none"|null, mutedUntil?: ISO|null } → the
// caller's own notify level for one channel and a timed mute, on their own
// membership row: { ok, level, mutedUntil }. `level` answers the level that
// applies (a NULL level is the channel type's default: a DM notifies on
// `all`, a channel on `mentions`). A member only; it UPDATEs the caller's own
// row and never inserts one, so it can never join anyone to a private
// channel (C24). A room whose roster is its board's or its team's has no row
// to set: 409. The producers read the level (talaria_notify::channel_listeners,
// docs/notifications/producers.md); `GET /api/channels/{id}` serves it back.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_body::{enum_msg, zod_type_name};
use talaria_channels::{channel_role, is_task_room};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{FOREVER_MS, parse_instant};
use talaria_session::require_user;
use talaria_state::AppState;

const LEVELS: &[&str] = &["all", "mentions", "none"];

/// The validated body: each member Option<"present">, its value Option<"set">.
#[derive(Debug, PartialEq, Eq)]
struct NotifyBody {
    level: Option<Option<String>>,
    muted_until: Option<Option<i64>>,
}

fn validate(obj: &serde_json::Map<String, Value>, now_ms: i64) -> Result<NotifyBody, String> {
    let level = match obj.get("level") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) if LEVELS.contains(&s.as_str()) => Some(Some(s.clone())),
        Some(_) => return Err(enum_msg(LEVELS)),
    };
    let muted_until = match obj.get("mutedUntil") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => match parse_instant(s) {
            Some(ms) if ms > now_ms && ms <= FOREVER_MS => Some(Some(ms)),
            Some(_) => return Err("mutedUntil must be in the future".into()),
            None => return Err("mutedUntil must be an ISO time, or null".into()),
        },
        Some(v) => {
            return Err(format!(
                "Invalid input: expected string, received {}",
                zod_type_name(v)
            ));
        }
    };
    if level.is_none() && muted_until.is_none() {
        return Err("nothing to update".into());
    }
    Ok(NotifyBody { level, muted_until })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[channels] role read on notify failed", e)),
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let b = match validate(obj, wall_ms()) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // UPDATE ONLY — an absent member keeps what is stored.
    let row: Option<(Option<String>, Option<i64>, String)> = match sqlx::query_as(
        "update channel_members cm \
            set notify_level = case when $3 then $4 else cm.notify_level end, \
                muted_until = case when $5 then to_timestamp($6::bigint / 1000.0) \
                                   else cm.muted_until end \
           from channels c \
          where c.id = cm.channel_id and cm.channel_id = $1::uuid and cm.user_id = $2::uuid \
         returning cm.notify_level, \
                   (trunc(extract(epoch from cm.muted_until) * 1000))::bigint, c.kind",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(b.level.is_some())
    .bind(b.level.clone().flatten())
    .bind(b.muted_until.is_some())
    .bind(b.muted_until.flatten())
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[channels] notify level write failed", e)),
    };
    let Some((level, muted_ms, kind)) = row else {
        // A member with no row of their own: the room's roster is someone
        // else's (a task's board, a team), and so are its notifications.
        let task_room = is_task_room(&state.pg, &id).await.unwrap_or(false);
        return Ok(house_error(
            StatusCode::CONFLICT,
            if task_room {
                "this room's notifications follow its board"
            } else {
                // S10: a team grant has no row of the person's own to hold a
                // level. Giving team members levels needs a per-person
                // table — an owner decision (docs/notifications/producers.md).
                "you're in this channel through a team, and per-person notification \
                 settings for team members aren't available yet"
            },
        ));
    };
    let effective =
        level.unwrap_or_else(|| if kind == "dm" { "all" } else { "mentions" }.to_string());
    Ok(Json(json!({
        "ok": true,
        "level": effective,
        "mutedUntil": muted_ms.map(epoch_ms_to_iso),
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(j: Value, now: i64) -> Result<NotifyBody, String> {
        validate(j.as_object().unwrap(), now)
    }

    #[test]
    fn the_body_is_pinned() {
        let now = 1_800_000_000_000;
        assert_eq!(
            v(json!({ "level": "none" }), now).unwrap(),
            NotifyBody {
                level: Some(Some("none".into())),
                muted_until: None
            }
        );
        assert_eq!(
            v(json!({ "level": null, "mutedUntil": null }), now).unwrap(),
            NotifyBody {
                level: Some(None),
                muted_until: Some(None)
            }
        );
        assert_eq!(
            v(json!({ "level": "loud" }), now).unwrap_err(),
            "Invalid option: expected one of \"all\"|\"mentions\"|\"none\""
        );
        assert_eq!(
            v(json!({ "mutedUntil": "2020-01-01T00:00:00.000Z" }), now).unwrap_err(),
            "mutedUntil must be in the future"
        );
        assert_eq!(
            v(json!({ "mutedUntil": "soon" }), now).unwrap_err(),
            "mutedUntil must be an ISO time, or null"
        );
        assert_eq!(
            v(json!({ "mutedUntil": 5 }), now).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert!(v(json!({ "mutedUntil": "9999-12-31T23:59:59.999Z" }), now).is_ok());
        assert_eq!(v(json!({}), now).unwrap_err(), "nothing to update");
    }
}

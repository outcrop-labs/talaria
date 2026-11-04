// /api/me/presence. The signed-in person's heartbeat.
//
// PUT with NO BODY (or one without a `clientId`) is wave 1's online ping,
// exactly: the client calls it every
// 30s while a Talaria tab is visible, and it sets `user:presence:{id}` with a
// 90s TTL; `GET /api/users` reads those keys to mark people online. Answers
// { ok: true }.
//
// PUT with a body is a client's presence heartbeat (I-17): `{clientId, kind:
// "web"|"desktop", state: "looking"|"active"|"idle"|"hidden", osIdleMs?}`. It
// records the client in the person's presence store, announces the winner on
// `user:<id>` when it changed, and sets the online key when the client is in
// use (`presence::marks_online`: a web tab unless hidden, the desktop shell
// only while its OS is looking or active — it heartbeats from the tray all
// night). Answers { ok, looking, active }.
//
// GET answers the store: { looking, active, clients: [{clientId, kind, state,
// at}] }, newest first. Phase 0 stores and announces; W2-E gates delivery on
// it (docs/notifications/delivery.md).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::now_ms as wall_ms;
use talaria_body::{enum_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_notify::presence::{self, ClientKind, ClientState, Heartbeat};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_users::{PRESENCE_TTL_S, presence_key};

const KINDS: &[&str] = &["web", "desktop"];
const STATES: &[&str] = &["looking", "active", "idle", "hidden"];

fn heartbeat(obj: &serde_json::Map<String, Value>) -> Result<Heartbeat, String> {
    let client_id = string_member(obj, "clientId", 8, 64)?;
    if !presence::valid_client_id(&client_id) {
        return Err("clientId must be 8-64 letters, digits, '-' or '_'".into());
    }
    let kind = enum_member(obj, "kind", KINDS)?;
    let state = enum_member(obj, "state", STATES)?;
    let os_idle_ms = match obj.get("osIdleMs") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .ok_or("osIdleMs must be a whole number of milliseconds")?,
        ),
    };
    Ok(Heartbeat {
        client_id,
        kind: ClientKind::parse(&kind).expect("the enum checked it"),
        state: ClientState::parse(&state).expect("the enum checked it"),
        os_idle_ms,
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // No body — or any body that names no `clientId`, which wave 1 ignored —
    // is wave 1's ping, byte for byte.
    let parsed = parse(&body);
    let beat = if parsed.get("clientId").is_none() {
        None
    } else {
        let obj = object_or_400(&parsed)?;
        match heartbeat(obj) {
            Ok(h) => Some(h),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    };
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[me/presence] redis unavailable", e)),
    };
    if beat.as_ref().is_none_or(presence::marks_online)
        && let Err(e) = redis::cmd("SET")
            .arg(presence_key(&user.id))
            .arg(1)
            .arg("EX")
            .arg(PRESENCE_TTL_S)
            .query_async::<()>(&mut conn)
            .await
    {
        return Ok(internal("[me/presence] ping failed", e));
    }
    let Some(h) = beat else {
        return Ok(Json(json!({ "ok": true })).into_response());
    };
    let deps = NotifyDeps::publishing(state.pg.clone(), Some(conn.clone()));
    match presence::record(&deps, &mut conn, &user.id, &h, wall_ms()).await {
        Ok(win) => Ok(Json(json!({
            "ok": true,
            "looking": win.looking,
            "active": win.active,
        }))
        .into_response()),
        Err(e) => Ok(internal("[me/presence] heartbeat failed", e)),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[me/presence] redis unavailable", e)),
    };
    match presence::snapshot(&mut conn, &user.id, wall_ms()).await {
        Ok(v) => Ok(Json(v).into_response()),
        Err(e) => Ok(internal("[me/presence] read failed", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hb(v: Value) -> Result<Heartbeat, String> {
        heartbeat(v.as_object().unwrap())
    }

    #[test]
    fn the_heartbeat_body_is_pinned() {
        let ok =
            hb(json!({ "clientId": "tab-0123abcd", "kind": "web", "state": "looking" })).unwrap();
        assert_eq!(ok.state, ClientState::Looking);
        assert_eq!(ok.os_idle_ms, None);
        let d = hb(json!({ "clientId": "desk_0123abcd", "kind": "desktop", "state": "active", "osIdleMs": 4000 }))
            .unwrap();
        assert_eq!((d.kind, d.os_idle_ms), (ClientKind::Desktop, Some(4000)));
        assert!(hb(json!({ "clientId": "short", "kind": "web", "state": "idle" })).is_err());
        assert_eq!(
            hb(json!({ "clientId": "has space here", "kind": "web", "state": "idle" }))
                .unwrap_err(),
            "clientId must be 8-64 letters, digits, '-' or '_'"
        );
        assert_eq!(
            hb(json!({ "clientId": "tab-0123abcd", "kind": "phone", "state": "idle" }))
                .unwrap_err(),
            "Invalid option: expected one of \"web\"|\"desktop\""
        );
        assert_eq!(
            hb(json!({ "clientId": "tab-0123abcd", "kind": "web", "state": "asleep" }))
                .unwrap_err(),
            "Invalid option: expected one of \"looking\"|\"active\"|\"idle\"|\"hidden\""
        );
        assert_eq!(
            hb(json!({ "clientId": "tab-0123abcd", "kind": "web", "state": "idle", "osIdleMs": -1 }))
                .unwrap_err(),
            "osIdleMs must be a whole number of milliseconds"
        );
    }
}

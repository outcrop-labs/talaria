// /api/kb/docs/{id}/live. Doc presence (the multiplayer layer's heartbeat).
// PUT { mode } → I'm here, viewing or editing. GET → who's here right now,
// with their mode — the doc header renders the avatar stack and the
// concurrent-edit warning from this. Redis keys kb:presence:<docId>:<userId>
// EX 45; heartbeats land every ~25s.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_api_facades::kb::comments::can_discuss_doc;
use talaria_body::{enum_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

fn key_prefix(doc_id: &str) -> String {
    format!("kb:presence:{doc_id}:")
}

const TTL: u64 = 45;

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let mode = match enum_member(obj, "mode", &["view", "edit"]) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[kb] presence redis failed", e)),
    };
    if let Err(e) = redis::cmd("SET")
        .arg(format!("{}{}", key_prefix(&id), user.id))
        .arg(mode)
        .arg("EX")
        .arg(TTL)
        .query_async::<()>(&mut conn)
        .await
    {
        return Ok(internal("[kb] presence write failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[kb] presence redis failed", e)),
    };
    let prefix = key_prefix(&id);
    let keys: Vec<String> = match redis::cmd("KEYS")
        .arg(format!("{prefix}*"))
        .query_async(&mut conn)
        .await
    {
        Ok(v) => v,
        Err(e) => return Ok(internal("[kb] presence scan failed", e)),
    };
    if keys.is_empty() {
        return Ok(Json(json!({ "active": [] })).into_response());
    }
    let modes: Vec<Option<String>> =
        match redis::cmd("MGET").arg(&keys).query_async(&mut conn).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[kb] presence read failed", e)),
        };
    let ids: Vec<String> = keys.iter().map(|k| k[prefix.len()..].to_string()).collect();
    let users: Vec<(String, Option<String>, Option<String>)> =
        match sqlx::query_as("select id::text, name, email from users where id = any($1::uuid[])")
            .bind(&ids)
            .fetch_all(&state.pg)
            .await
        {
            Ok(v) => v,
            Err(e) => return Ok(internal("[kb] presence users failed", e)),
        };
    let active: Vec<Value> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let u = users.iter().find(|(uid, _, _)| uid == id)?;
            Some(json!({
                "userId": id,
                "name": u.1.as_deref().or(u.2.as_deref()).unwrap_or("someone"),
                "mode": if modes.get(i).and_then(|m| m.as_deref()) == Some("edit") { "edit" } else { "view" },
            }))
        })
        .collect();
    Ok(Json(json!({ "active": active })).into_response())
}

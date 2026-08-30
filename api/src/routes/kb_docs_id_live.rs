// /api/kb/docs/{id}/live — port of ui/src/routes/api/kb.docs.$id.live.ts.
// Doc presence (the multiplayer layer's heartbeat). PUT { mode } → I'm here,
// viewing or editing. GET → who's here right now, with their mode — the doc
// header renders the avatar stack and the concurrent-edit warning from this.
// Redis keys kb:presence:<docId>:<userId> EX 45; heartbeats land every ~25s.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::body::{as_object, enum_member, parse};
use crate::error::house_error;
use crate::kb_comments::can_discuss_doc;
use crate::session::{require_user, who_of};
use crate::state::AppState;

fn key_prefix(doc_id: &str) -> String {
    format!("kb:presence:{doc_id}:")
}

const TTL: u64 = 45;

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
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return house_error(StatusCode::NOT_FOUND, "not found");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mode = match enum_member(obj, "mode", &["view", "edit"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[kb] presence redis failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if let Err(e) = redis::cmd("SET")
        .arg(format!("{}{}", key_prefix(&id), user.id))
        .arg(mode)
        .arg("EX")
        .arg(TTL)
        .query_async::<()>(&mut conn)
        .await
    {
        tracing::error!("[kb] presence write failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return house_error(StatusCode::NOT_FOUND, "not found");
    }
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[kb] presence redis failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let prefix = key_prefix(&id);
    let keys: Vec<String> = match redis::cmd("KEYS")
        .arg(format!("{prefix}*"))
        .query_async(&mut conn)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] presence scan failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if keys.is_empty() {
        return Json(json!({ "active": [] })).into_response();
    }
    let modes: Vec<Option<String>> =
        match redis::cmd("MGET").arg(&keys).query_async(&mut conn).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] presence read failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        };
    let ids: Vec<String> = keys.iter().map(|k| k[prefix.len()..].to_string()).collect();
    let users: Vec<(String, Option<String>, Option<String>)> =
        match sqlx::query_as("select id::text, name, email from users where id = any($1::uuid[])")
            .bind(&ids)
            .fetch_all(&state.pg)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] presence users failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
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
    Json(json!({ "active": active })).into_response()
}

// /api/fleet/agents/{id}/secrets. Per-agent secrets, write-only. GET →
// names + timestamps (never values). PUT { name, value } → set/replace.
// DELETE { name } → remove. Admin, or the owner of a personal assistant.
// Takes effect on the next start from Talaria. Every write audits — secret
// NAMES only, never values.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_secrets::{delete_agent_secret, list_agent_secrets, set_agent_secret};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{as_object, parse, string_member, trimmed_string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_personal_agent::owns_agent;
use talaria_session::{actor_of, require_user, secretbox_or_500};
use talaria_state::AppState;

async fn gate(state: &AppState, user_id: &str, role: &str, id: &str) -> bool {
    role == "admin" || owns_agent(&state.pg, user_id, None, Some(id)).await
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !gate(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(match list_agent_secrets(&state.pg, &id).await {
        Ok(secrets) => Json(json!({ "secrets": secrets })).into_response(),
        Err(e) => internal("[fleet] list_agent_secrets failed", e),
    })
}

pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !gate(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match trimmed_string_member(obj, "name", 2, 64) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // value is UNtrimmed — a leading space is a legal secret character.
    let value = match string_member(obj, "value", 1, 8192) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[fleet] secretbox failed").await?;
    let actor = user.email.clone().or_else(|| user.name.clone());
    Ok(
        match set_agent_secret(&state.pg, &sb, &id, &name, &value, actor.as_deref()).await {
            Ok(()) => {
                audit(
                    &state,
                    &user,
                    "agent.secret_set",
                    &id,
                    json!({ "name": name }),
                );
                Json(json!({ "ok": true })).into_response()
            }
            Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
        },
    )
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !gate(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    // Body { name } — matches PUT's transport. When the body doesn't parse
    // (unparseable JSON, wrong shape, failed validation), the old ?name=
    // query spelling still works, and THAT path is only length-checked by
    // truthiness.
    let body_name = as_object(&parse(&body))
        .ok()
        .and_then(|obj| string_member(obj, "name", 1, 64).ok());
    let name = match body_name {
        Some(n) => n,
        None => match query_param(&uri, "name") {
            Some(n) if !n.is_empty() => n,
            _ => return Ok(house_error(StatusCode::BAD_REQUEST, "missing name")),
        },
    };
    if let Err(e) = delete_agent_secret(&state.pg, &id, &name).await {
        return Ok(internal("[fleet] query_param failed", e));
    }
    audit(
        &state,
        &user,
        "agent.secret_delete",
        &id,
        json!({ "name": name }),
    );
    Ok(Json(json!({ "ok": true })).into_response())
}

/// Query lookup — the FIRST value for the key.
fn query_param(uri: &Uri, key: &str) -> Option<String> {
    uri.query().and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            if k == key {
                Some(percent_decode(v))
            } else {
                None
            }
        })
    })
}

// spellings (%20, %2F) plus '+'-as-space.
fn percent_decode(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                // get() handles the trailing-% boundary; a bad hex pair
                // passes through verbatim.
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|s| u8::from_str_radix(s, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The family's audit — names only, never values, fire-and-forget.
fn audit(
    state: &AppState,
    user: &talaria_session::SessionUser,
    action: &str,
    id: &str,
    after: serde_json::Value,
) {
    let actor = actor_of(user);
    let action = action.to_string();
    let id = id.to_string();
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: &action,
                target_type: "agent",
                target_id: Some(&id),
                target_label: None,
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
}

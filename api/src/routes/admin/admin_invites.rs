// /api/admin/invites. Invites. GET → recent invites with state. POST
// { email } → create + send (re-invites re-issue with a fresh token).
// DELETE { id } → revoke.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, string_member, uuid_member};
use crate::error::{house_error, thrown_internal_error};
use crate::invites::{create_invite, list_invites, revoke_invite};
use crate::session::{SessionUser, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

fn invite_actor(user: &SessionUser) -> String {
    user.email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".to_string())
}

/// The request's ORIGIN header — the browser's own origin — as the invite
/// email's fallback base.
fn origin_of(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    Json(serde_json::json!({ "invites": list_invites(&state.pg).await })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = invite_actor(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // email max 200 — the empty string is legal; create_invite itself
    // answers it.
    let email = match string_member(obj, "email", 0, 200) {
        Ok(e) => e,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/invites] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    match create_invite(
        &state.pg,
        &sb,
        &email,
        &actor,
        origin_of(&headers).as_deref(),
    )
    .await
    {
        Ok((invite, email_sent, email_error)) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "invite.create",
                    target_type: "invite",
                    target_id: invite.get("id").and_then(Value::as_str),
                    target_label: invite.get("email").and_then(Value::as_str),
                    before: None,
                    after: None,
                },
            )
            .await;
            // emailError is omitted until it exists.
            let mut out = serde_json::Map::new();
            out.insert("invite".into(), invite);
            out.insert("emailSent".into(), Value::Bool(email_sent));
            if let Some(e) = email_error {
                out.insert("emailError".into(), Value::String(e));
            }
            Json(Value::Object(out)).into_response()
        }
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // { id }: type message first, then "Invalid UUID".
    let id = match uuid_member(obj, "id") {
        Ok(i) => i,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = revoke_invite(&state.pg, &id).await {
        tracing::error!("[admin/invites] revoke failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &crate::session::actor_of(&user),
            action: "invite.revoke",
            target_type: "invite",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Json(serde_json::json!({ "ok": true })).into_response()
}

// /api/admin/secrets. The secrets inventory. GET is a VIEW over the stores
// that own each value — presence, provenance and readability, never the
// value itself. DELETE clears one row's ciphertext, or every row that
// cannot be read.
//
// This is the in-app half of `talaria reset secrets`. The CLI clears
// everything sealed because it cannot tell what is broken; this can, so it
// clears only that.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::secret_health::{ClearError, clear_secret, clear_unreadable, secret_health};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/secrets] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    Json(secret_health(&state.pg, &sb, &state.cfg.secret_root).await).into_response()
}

/// DELETE body: { id: string 1..200 } or { unreadable: true }.
pub async fn delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // dispatch by key presence, each branch answering its own words — the
    // id path is checked first (it wins when both would parse, extra keys
    // stripped), and neither branch surviving is "Invalid input".
    let id = match obj.get("id") {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) => {
                let n = crate::body::utf16_len(s);
                if n < 1 {
                    return house_error(StatusCode::BAD_REQUEST, &crate::body::too_small_msg(1));
                }
                if n > 200 {
                    return house_error(StatusCode::BAD_REQUEST, &crate::body::too_big_msg(200));
                }
                Some(s.to_string())
            }
            None => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &crate::body::string_msg(crate::body::zod_type_name(v)),
                );
            }
        },
    };
    if let Some(id) = id {
        match clear_secret(&state.pg, &id).await {
            Ok(changed) => {
                // Audit the attempt either way: "an admin tried to clear
                // this" is the fact worth keeping, and a no-op still tells
                // you someone was here.
                log_audit(
                    &state.pg,
                    AuditEntry {
                        actor: &actor,
                        action: "secret.clear",
                        target_type: "secret",
                        target_id: Some(&id),
                        target_label: None,
                        before: None,
                        after: Some(serde_json::json!({ "changed": changed })),
                    },
                )
                .await;
                Json(serde_json::json!({ "ok": true, "changed": changed })).into_response()
            }
            Err(ClearError::Unknown) => house_error(StatusCode::NOT_FOUND, "unknown secret"),
            Err(_) => {
                // Never echo the raw error: these paths sit next to key
                // material.
                tracing::error!("[admin/secrets] clear failed {id}");
                house_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not clear that secret \u{2014} see server logs",
                )
            }
        }
    } else if obj.get("unreadable") == Some(&Value::Bool(true)) {
        // must be exactly true — anything else falls to the blanket below.
        let sb = match state.secretbox().await {
            Ok(sb) => sb,
            Err(e) => {
                tracing::error!("[admin/secrets] secretbox unavailable: {e}");
                return thrown_internal_error();
            }
        };
        let (cleared, failed) = clear_unreadable(&state.pg, &sb, &state.cfg.secret_root).await;
        let after = serde_json::json!({ "cleared": cleared, "failed": failed });
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "secret.clear-unreadable",
                target_type: "secrets",
                target_id: None,
                target_label: Some(&format!("{} cleared", cleared.len())),
                before: None,
                after: Some(after.clone()),
            },
        )
        .await;
        let mut out = serde_json::Map::new();
        out.insert("ok".into(), Value::Bool(true));
        if let (Some(c), Some(f)) = (after.get("cleared"), after.get("failed")) {
            out.insert("cleared".into(), c.clone());
            out.insert("failed".into(), f.clone());
        }
        Json(Value::Object(out)).into_response()
    } else {
        house_error(StatusCode::BAD_REQUEST, "Invalid input")
    }
}

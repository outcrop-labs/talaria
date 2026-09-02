// /api/admin/encryption. Encryption status + one-click key rotation.
// Rotating re-generates the data key and re-encrypts every stored secret
// (provider keys, agent secrets, OAuth tokens) in a single pass — one
// action, no per-secret steps.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse};
use crate::config::RootSource;
use crate::error::{house_error, thrown_internal_error};
use crate::secret_rotation::rotate_secrets;
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
    let active: Result<Option<(i32, i64)>, sqlx::Error> = sqlx::query_as(
        "select version, (trunc(extract(epoch from created_at) * 1000))::bigint \
         from secret_keys where active order by version desc limit 1",
    )
    .fetch_optional(&state.pg)
    .await;
    let counts: Result<(i64,), sqlx::Error> = sqlx::query_as(
        "select (select count(*) from llm_endpoints where api_key_cipher is not null) \
              + (select count(*) from agent_secrets where value_enc is not null) \
              + (select count(*) from google_connections) \
              + (select count(*) from google_org_connection) as n",
    )
    .fetch_one(&state.pg)
    .await;
    let (active, counts) = match (active, counts) {
        (a, Ok(c)) => (a.ok().flatten(), c.0),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!("[admin/encryption] status read failed: {e}");
            return thrown_internal_error();
        }
    };
    let root_source = match state.cfg.secret_root.source() {
        RootSource::SecretKey => "env:TALARIA_SECRET_KEY",
        RootSource::SecretKeyFile => "key-file",
        RootSource::AuthSecretFallback => "env:AUTH_SECRET",
    };
    Json(serde_json::json!({
        "keyVersion": active.as_ref().map(|(v, _)| *v),
        "rotatedAt": active
            .map(|(_, ms)| crate::agent_auth::epoch_ms_to_iso(ms)),
        "secretCount": counts,
        "rootSource": root_source,
        "algorithm": "AES-256-GCM (envelope; post-quantum-safe symmetric)",
    }))
    .into_response()
}

/// The POST body is optional: a bodyless POST (or null) is a plain rotation;
/// an object may carry `newRootSecret`.
pub async fn post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let mut new_root: Option<String> = None;
    match &parsed {
        Value::Null => {}
        v => {
            let o = match as_object(v) {
                Ok(o) => o,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // min 16 (the custom sentence), max 400 (the standard one).
            if let Some(v) = o.get("newRootSecret") {
                let Some(s) = v.as_str() else {
                    return house_error(
                        StatusCode::BAD_REQUEST,
                        &crate::body::string_msg(crate::body::zod_type_name(v)),
                    );
                };
                if crate::body::utf16_len(s) < 16 {
                    return house_error(
                        StatusCode::BAD_REQUEST,
                        "new root secret must be at least 16 chars",
                    );
                }
                if crate::body::utf16_len(s) > 400 {
                    return house_error(StatusCode::BAD_REQUEST, &crate::body::too_big_msg(400));
                }
                new_root = Some(s.to_string());
            }
        }
    }

    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/encryption] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    match rotate_secrets(&state.pg, &sb, new_root.as_deref()).await {
        Ok((fresh, result)) => {
            // The caller installs the new key set only after the DB
            // transaction committed — a failure above leaves the in-memory
            // keys and the database in their old, consistent state.
            state.install_secretbox(fresh);
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor_of(&user),
                    action: "encryption.rotate",
                    target_type: "secret_keys",
                    target_id: None,
                    target_label: Some(&format!("v{}", result.version)),
                    before: None,
                    after: Some(serde_json::json!({
                        "reencrypted": result.reencrypted,
                        "rootRewrapped": result.root_rewrapped,
                    })),
                },
            )
            .await;
            let mut out = serde_json::Map::new();
            out.insert("ok".into(), Value::Bool(true));
            out.insert("version".into(), serde_json::json!(result.version));
            out.insert("reencrypted".into(), serde_json::json!(result.reencrypted));
            out.insert(
                "rootRewrapped".into(),
                serde_json::json!(result.root_rewrapped),
            );
            Json(Value::Object(out)).into_response()
        }
        Err(_) => {
            // Server log gets the real error; the client gets a generic
            // line — crypto failure messages must never risk echoing key
            // material.
            tracing::error!("[encryption.rotate] failed");
            house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rotation failed (no secrets changed) \u{2014} see server logs",
            )
        }
    }
}

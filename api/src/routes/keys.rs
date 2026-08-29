// /api/keys — port of ui/src/routes/api/keys.ts. Personal API keys for the
// Talaria LLM gateway. GET → my keys + whether I may mint. POST → mint one;
// the secret is in THIS response only (never stored, never logged, never in
// the audit entry).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, string_member};
use crate::error::house_error;
use crate::llm_keys::{can_mint_keys, list_keys, mint_key};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let keys = match list_keys(&state.pg, &user.id).await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("[keys] list failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let can_mint = match can_mint_keys(&state.pg, &user.id, &user.role).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[keys] can-mint read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    Json(json!({ "keys": keys, "canMint": can_mint })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match can_mint_keys(&state.pg, &user.id, &user.role).await {
        Ok(true) => {}
        Ok(false) => {
            return house_error(
                StatusCode::FORBIDDEN,
                "API keys are not enabled for your account — ask an admin",
            );
        }
        Err(e) => {
            tracing::error!("[keys] can-mint read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 60) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (key, secret) = match mint_key(&state.pg, &user.id, &name).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[keys] mint failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "key.mint",
            target_type: "llm-key",
            target_id: Some(&key.id),
            target_label: Some(&name),
            before: None,
            after: None,
        },
    )
    .await;
    Json(json!({ "key": key, "secret": secret })).into_response()
}

// /api/keys. Personal API keys for the Talaria LLM gateway. GET → my keys +
// whether I may mint. POST → mint one; the secret is in THIS response only
// (never stored, never logged, never in the audit entry).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_llm_keys::{can_mint_keys, list_keys, mint_key};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let keys = match list_keys(&state.pg, &user.id).await {
        Ok(k) => k,
        Err(e) => return Ok(internal("[keys] list failed", e)),
    };
    let can_mint = match can_mint_keys(&state.pg, &user.id, &user.role).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[keys] can-mint read failed", e)),
    };
    Ok(Json(json!({ "keys": keys, "canMint": can_mint })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    match can_mint_keys(&state.pg, &user.id, &user.role).await {
        Ok(true) => {}
        Ok(false) => {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "API keys are not enabled for your account — ask an admin",
            ));
        }
        Err(e) => return Ok(internal("[keys] can-mint read failed", e)),
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match string_member(obj, "name", 1, 60) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let (key, secret) = match mint_key(&state.pg, &user.id, &name).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[keys] mint failed", e)),
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
    Ok(Json(json!({ "key": key, "secret": secret })).into_response())
}

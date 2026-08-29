// /api/keys/{id} — port of ui/src/routes/api/keys.$id.ts. DELETE revokes one
// of MY keys (the hash stays for audit); PUT sets my key's self-imposed
// policy (#265). A non-uuid {id} is a recorded divergence: TS's raw SQL bind
// throws and its platform answers a plain-text 500; this port logs and
// answers the house envelope (RUST-MIGRATION.md, divergences — the
// google-client precedent).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{NumKind, as_object, nullable_number_member, parse};
use crate::error::house_error;
use crate::llm_keys::{KeyPolicy, revoke_key, set_key_policy};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

/// TS reaches Postgres with the raw path param and lets the $id::uuid bind
/// reject garbage; here the parse is just the gate — the original string
/// flows on to the query and the audit row verbatim, as params.id does.
/// Some(gate) = the 500 to return.
fn uuid_or_500(id: &str, action: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    tracing::error!("[keys] non-uuid id on {action}: {id:?}");
    Some(house_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error",
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_or_500(&id, "DELETE") {
        return gate;
    }
    if let Err(e) = revoke_key(&state.pg, &user.id, &id).await {
        tracing::error!("[keys] revoke failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "key.revoke",
            target_type: "llm-key",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}

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
    // Validation before the uuid gate, like TS: parseBody runs before
    // setKeyPolicy ever reaches the bind, so a bad body wins the 400.
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let spend_cap_tokens =
        match nullable_number_member(obj, "spendCapTokens", NumKind::Int, 0.0, 1e15) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
    let spend_cap_usd = match nullable_number_member(obj, "spendCapUsd", NumKind::Float, 0.0, 1e9) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let rate_limit_per_minute =
        match nullable_number_member(obj, "rateLimitPerMinute", NumKind::Int, 0.0, 10_000.0) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
    if let Some(gate) = uuid_or_500(&id, "PUT") {
        return gate;
    }
    // The echo: `body.x ?? null` — absent and null both answer null, but a
    // literal 0 STAYS 0 in the response and the audit row even though the
    // write normalizes it to unlimited. js_num so 1000 echoes as "1000".
    let policy_json = json!({
        "spendCapTokens": spend_cap_tokens.map(crate::body::js_num),
        "spendCapUsd": spend_cap_usd.map(crate::body::js_num),
        "rateLimitPerMinute": rate_limit_per_minute.map(crate::body::js_num),
    });
    let set = match set_key_policy(
        &state.pg,
        &user.id,
        &id,
        KeyPolicy {
            spend_cap_tokens,
            spend_cap_usd,
            rate_limit_per_minute,
        },
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[keys] policy write failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if !set {
        return house_error(StatusCode::NOT_FOUND, "no such key");
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "key.policy",
            target_type: "llm-key",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(policy_json.clone()),
        },
    )
    .await;
    Json(json!({ "ok": true, "policy": policy_json })).into_response()
}

// The harness's rate-limit blocks, persisted through the broker.
//
//   POST   /api/workbench/auth/v1/credential/{id}/block   remember one
//   DELETE /api/workbench/auth/v1/credential/{id}/block   forget one
//   DELETE /api/workbench/auth/v1/credential/{id}/blocks  forget all
//
// These are CACHE, not credentials: omp learned that a provider is refusing
// until some time, and writes it here so the next job does not have to learn
// it again. A lost row costs one wasted upstream call, which is why these are
// forgiving about shape where the snapshot is strict.
//
// `blockScope` is '' for a provider-wide block and a meter name (Codex's
// `chat` / `spark`) otherwise — the capability header the client sends on
// every snapshot request is it announcing that it understands those scopes.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_body::{
    NumKind, nullable_number_member, optional_max_string_member, parse, string_member,
};
use talaria_coding_accounts as ca;
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

use super::coding_broker::{broker_agent, owned_account};

/// The largest integer the client's JSON can carry exactly.
const SAFE_INT: f64 = 9_007_199_254_740_991.0;

// doc: Remember that a provider is rate-limiting one of this agent's coding
// doc: accounts until a given time, so the next job does not rediscover it.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Response, Response> {
    let agent_id = broker_agent(&state, &headers).await?;
    owned_account(&state, &agent_id, id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let provider_key = match string_member(obj, "providerKey", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Err(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // An absent scope is the provider-wide block, which is the common case.
    let block_scope = optional_max_string_member(obj, "blockScope", 200)
        .ok()
        .flatten()
        .unwrap_or_default();
    // Epoch ms, bounded by the JS safe integer the client sends them as.
    let blocked_until_ms =
        match nullable_number_member(obj, "blockedUntilMs", NumKind::Int, 0.0, SAFE_INT) {
            Ok(Some(v)) => v as i64,
            Ok(None) => {
                return Err(house_error(
                    StatusCode::BAD_REQUEST,
                    "blockedUntilMs is required",
                ));
            }
            Err(msg) => return Err(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
    let updated_at_ms = nullable_number_member(obj, "updatedAtMs", NumKind::Int, 0.0, SAFE_INT)
        .ok()
        .flatten()
        .map(|v| v as i64);
    ca::put_block(
        &state.pg,
        id,
        &provider_key,
        &block_scope,
        blocked_until_ms,
        updated_at_ms,
    )
    .await
    .map_err(|e| internal("[workbench/auth] block write failed", e))?;
    Ok(axum::Json(json!({ "ok": true })).into_response())
}

// doc: Forget one remembered rate-limit block. An empty `blockScope` targets
// doc: the provider-wide row.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Response, Response> {
    let agent_id = broker_agent(&state, &headers).await?;
    owned_account(&state, &agent_id, id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let provider_key = match string_member(obj, "providerKey", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Err(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let block_scope = optional_max_string_member(obj, "blockScope", 200)
        .ok()
        .flatten()
        .unwrap_or_default();
    ca::clear_blocks(&state.pg, id, Some((&provider_key, &block_scope)))
        .await
        .map_err(|e| internal("[workbench/auth] block clear failed", e))?;
    Ok(axum::Json(json!({ "ok": true })).into_response())
}

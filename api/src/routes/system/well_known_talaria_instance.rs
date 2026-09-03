// /api/well-known/talaria-instance.
//
// Instance identity beacon — the target of hosting-domain verification's
// self-fetch, and the SPA's one boot-time read of the instance's display
// name (the tab title). Public and harmless: a random UUID that proves
// which deployment answered, and the name the operator chose for it —
// nothing more. A failed name read folds into null (the settings read's
// own fallback), so the verifier's round-trip never breaks on it.

use crate::instance::{get_company_name, get_instance_id};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>) -> Response {
    let id = match get_instance_id(&state.pg).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[well-known] instance id read failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    Json(json!({ "instance": id, "companyName": get_company_name(&state.pg).await }))
        .into_response()
}

// /api/well-known/talaria-instance.
//
// Instance identity beacon — the target of hosting-domain verification's
// self-fetch. Public and harmless: a random UUID that proves which deployment
// answered, nothing more.

use crate::instance::get_instance_id;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>) -> Response {
    match get_instance_id(&state.pg).await {
        Ok(id) => Json(json!({ "instance": id })).into_response(),
        Err(e) => {
            tracing::error!("[well-known] instance id read failed: {e}");
            crate::error::thrown_internal_error()
        }
    }
}

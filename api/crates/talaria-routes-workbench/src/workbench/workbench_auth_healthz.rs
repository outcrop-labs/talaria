// /api/workbench/auth/v1/healthz. omp's auth-broker liveness probe — the one
// route in the protocol that carries no bearer, by the protocol's own
// definition. It says nothing about any agent, so there is nothing to leak.

use axum::Json;
use axum::response::{IntoResponse, Response};
use serde_json::json;

// doc: Liveness for the agent-facing coding-account broker. Unauthenticated by
// doc: protocol definition; reveals nothing beyond "this instance speaks it".
pub async fn get() -> Response {
    Json(json!({ "ok": true, "version": "1" })).into_response()
}

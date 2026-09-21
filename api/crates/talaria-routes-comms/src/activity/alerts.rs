// /api/alerts. GET → derived system alerts (no persistence) for the
// requesting user.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_alerts::compute_alerts;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(json!({ "alerts": compute_alerts(&state, &user.id).await })).into_response())
}

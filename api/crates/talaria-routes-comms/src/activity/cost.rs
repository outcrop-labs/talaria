// GET /api/cost. The token ledger overview (totals, per-agent, per-day).
// Org-wide financials: admins + people granted the Observability view.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_api_facades::gateway::usage::cost_overview;
use talaria_error::internal;
use talaria_session::require_view;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_view(&state, &headers, "/observability").await?;
    Ok(match cost_overview(&state.pg).await {
        // No envelope — the overview object IS the body.
        Ok(overview) => Json(overview).into_response(),
        Err(e) => internal("[cost] overview query failed", e),
    })
}

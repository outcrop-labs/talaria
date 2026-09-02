// GET /api/cost. The token ledger overview (totals, per-agent, per-day).
// Org-wide financials: admins + people granted the Observability view.

use crate::error::thrown_internal_error;
use crate::gateway::usage::cost_overview;
use crate::session::require_view;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_view(&state, &headers, "/observability").await {
        return gate;
    }
    match cost_overview(&state.pg).await {
        // No envelope — the overview object IS the body.
        Ok(overview) => Json(overview).into_response(),
        Err(e) => {
            tracing::error!("[cost] overview query failed: {e}");
            thrown_internal_error()
        }
    }
}

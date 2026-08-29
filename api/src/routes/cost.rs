// GET /api/cost — port of ui/src/routes/api/cost.ts. The token ledger
// overview (totals, per-agent, per-day). Org-wide financials: admins + people
// granted the Observability view.

use crate::error::house_error;
use crate::gateway::usage::cost_overview;
use crate::session::require_view;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_view(&state, &headers, "/observability").await {
        return gate;
    }
    match cost_overview(&state.pg).await {
        // No envelope — the overview object IS the body, like TS.
        Ok(overview) => Json(overview).into_response(),
        Err(e) => {
            tracing::error!("[cost] overview query failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

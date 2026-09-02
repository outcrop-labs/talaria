// /api/home — port of ui/src/routes/api/home.ts.
// GET → the Home/Today summary for the signed-in user.
//
// The TS file carries a side-effect import of server/digest.ts (the job
// registration handshake with the bun server entry). Rust has no module-load
// side effects to lean on — the digest job registers in main.rs's scheduler
// table with every other job, which is the honest version of the same trick.

use crate::home::home_summary;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match home_summary(&state, &user.id, user.role == "admin").await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => {
            tracing::error!("[home] summary failed: {e}");
            crate::error::thrown_internal_error()
        }
    }
}

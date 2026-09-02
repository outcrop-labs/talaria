// GET /api/apps. The signed-in view of installed apps: ENABLED apps only,
// manifest data the client needs to draw nav items, routes, and settings tabs.
// Per-user view gating happens client-side off deniedViews (and server-side at
// the app API gateway) — this list is not secret, it is the platform's own menu.

use crate::session::require_user;
use crate::state::AppState;
use crate::users::enabled_apps;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct AppsBody {
    apps: Vec<crate::users::WireApp>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    Json(AppsBody {
        apps: enabled_apps(&state.pg).await,
    })
    .into_response()
}

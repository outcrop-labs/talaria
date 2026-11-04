// GET /api/apps. The signed-in view of installed apps: ENABLED apps only,
// manifest data the client needs to draw nav items, routes, and settings tabs.
// Per-user view gating happens client-side off the session's appGrants (and
// server-side at the app API gateway and the /app-builds/ door) — this list is
// not secret, it is the platform's own menu.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_users::enabled_apps;

#[derive(serde::Serialize)]
struct AppsBody {
    apps: Vec<talaria_users::WireApp>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_user(&state, &headers).await?;
    Ok(Json(AppsBody {
        apps: enabled_apps(&state.pg).await,
    })
    .into_response())
}

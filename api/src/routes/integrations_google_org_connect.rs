// GET /api/integrations/google/org/connect — port of
// ui/src/routes/api/integrations/google.org.connect.ts. An admin begins
// connecting the shared org Google account (offline access). Wider scopes
// than the per-user flow: the org account is the one that provisions the
// shared calendar + Drive.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::error::house_error;
use crate::google_client::resolve_google_client;
use crate::google_oauth::{
    ORG_CONNECT_SCOPES, google_connect_url, google_integration_enabled,
    google_org_connect_redirect_uri,
};
use crate::session::{get_session_user, random_token, state_cookie};
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match get_session_user(&state, &headers).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("[integrations/google/org] session read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(user) = user else {
        return (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response();
    };
    if user.role != "admin" {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let sb = state.secretbox().await.unwrap_or_default();
    if !google_integration_enabled(&state.pg, &sb).await {
        return house_error(
            StatusCode::BAD_REQUEST,
            "Google integration is not configured",
        );
    }
    let Some(cfg) = resolve_google_client(&state.pg, &sb).await else {
        tracing::error!("[integrations/google/org] integration enabled but no client resolved");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    };
    let public_url = crate::auth_config::get_auth_config().public_url;
    let state_token = random_token();
    let url = google_connect_url(
        &cfg,
        &google_org_connect_redirect_uri(public_url.as_deref(), &headers, &uri),
        &state_token,
        ORG_CONNECT_SCOPES,
    );
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, url),
            (header::SET_COOKIE, state_cookie(&state_token)),
        ],
    )
        .into_response()
}

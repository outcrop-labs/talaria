// GET /api/integrations/google/connect — begin the offline-access consent
// dance: set the one-shot state cookie and 302 to Google. Requires an
// authenticated session (the connection binds to this user) — checked BEFORE
// the enabled gate, unlike the callback, which checks the gate first. Like the
// login dance, a pinned public origin owns the flow: a start arriving
// elsewhere relocates there first, so the state cookie lands where the
// callback will read it.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::error::{house_error, thrown_internal_error};
use crate::google::client::resolve_google_client;
use crate::google::oauth::{
    WORKSPACE_SCOPES, google_connect_redirect_uri, google_connect_url, google_integration_enabled,
    oauth_relocation,
};
use crate::session::{get_session_user, random_token, state_cookie_for};
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match get_session_user(&state, &headers).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("[integrations/google] session read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(_user) = user else {
        return (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response();
    };
    let sb = state.secretbox().await.unwrap_or_default();
    if !google_integration_enabled(&state.pg, &sb).await {
        return house_error(
            StatusCode::BAD_REQUEST,
            "Google integration is not configured",
        );
    }
    let Some(cfg) = resolve_google_client(&state.pg, &sb).await else {
        // The enabled check and the client resolve together; landing here
        // without a client is not a state either runtime built.
        tracing::error!("[integrations/google] integration enabled but no client resolved");
        return thrown_internal_error();
    };
    let public_url = crate::auth_config::get_auth_config().public_url;
    if let Some(to) = oauth_relocation(
        public_url.as_deref(),
        &headers,
        &uri,
        "/api/integrations/google/connect",
    ) {
        return (StatusCode::FOUND, [(header::LOCATION, to)]).into_response();
    }
    let state_token = random_token();
    let url = google_connect_url(
        &cfg,
        &google_connect_redirect_uri(public_url.as_deref(), &headers, &uri),
        &state_token,
        WORKSPACE_SCOPES,
    );
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, url),
            (header::SET_COOKIE, state_cookie_for(&headers, &state_token)),
        ],
    )
        .into_response()
}

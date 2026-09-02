// GET /api/auth/google. Begin the OAuth dance: set the one-shot state cookie
// and 302 to Google's consent screen. The state cookie is double-submit CSRF
// proof — random in, random back, compared constant-time at the callback.
//
// A pinned public origin (AUTH_PUBLIC_URL) owns the dance: when the request
// arrived at a different origin (a LAN URL, a probe host), the state cookie
// set HERE would live on that origin while Google returns to the pinned one —
// a guaranteed bad_state loop. Relocate first, so the cookie is set where the
// callback will land.

use crate::error::{house_error, thrown_internal_error};
use crate::google::client::{google_login_enabled, resolve_google_client};
use crate::google::oauth::{google_auth_url, google_redirect_uri, oauth_relocation};
use crate::session::{random_token, state_cookie_for};
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let sb = state.secretbox().await.unwrap_or_default();
    if !google_login_enabled(&state.pg, &sb).await {
        return house_error(StatusCode::BAD_REQUEST, "Google login is disabled");
    }
    let Some(cfg) = resolve_google_client(&state.pg, &sb).await else {
        // The toggle and the client are gated together in google_login_enabled;
        // reaching here without a client is not a state either runtime built.
        tracing::error!("[auth/google] login enabled but no client resolved");
        return thrown_internal_error();
    };
    let public_url = crate::auth_config::get_auth_config().public_url;
    // The pinned origin's own start URL — one hop, then this handler runs at
    // home and every origin-derived thing agrees with the browser.
    if let Some(to) = oauth_relocation(public_url.as_deref(), &headers, &uri, "/api/auth/google") {
        return (StatusCode::FOUND, [(header::LOCATION, to)]).into_response();
    }
    let state_token = random_token();
    let url = google_auth_url(
        &cfg,
        &google_redirect_uri(public_url.as_deref(), &headers, &uri),
        &state_token,
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

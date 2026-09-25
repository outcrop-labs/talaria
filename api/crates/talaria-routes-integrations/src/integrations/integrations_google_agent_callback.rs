// GET /api/integrations/google/agent/callback — store the agent's own Google
// connection and set its principal to that identity. The state cookie is the
// CSRF check; the state row says which agent the admin started this for.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use talaria_agent_auth::now_ms;
use talaria_api_facades::google::agent::{
    PrincipalKind, SaveAgentConnection, save_agent_connection, take_agent_oauth_state,
    upsert_principal,
};
use talaria_api_facades::google::oauth::{
    WORKSPACE_SCOPES, exchange_google_tokens, google_agent_connect_redirect_uri,
    google_integration_enabled, oauth_relocation, query_pairs,
};
use talaria_body::percent_encode;
use talaria_error::internal;
use talaria_session::{STATE_COOKIE, require_perm, state_matches};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let back = |status: &str| {
        let mut res = (
            StatusCode::FOUND,
            [(
                header::LOCATION,
                format!("/agents?googleAgent={}", percent_encode(status)),
            )],
        )
            .into_response();
        if let Ok(v) =
            axum::http::HeaderValue::from_str(&talaria_session::clear_state_cookie_for(&headers))
        {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
        res
    };

    let path_and_query = format!(
        "{}{}",
        uri.path(),
        uri.query().map(|q| format!("?{q}")).unwrap_or_default()
    );
    if let Some(to) = oauth_relocation(
        talaria_auth_config::get_auth_config().public_url.as_deref(),
        &headers,
        &uri,
        &path_and_query,
    ) {
        return (StatusCode::FOUND, [(header::LOCATION, to)]).into_response();
    }

    let sb = state.secretbox().await.unwrap_or_default();
    if !google_integration_enabled(&state.pg, &sb).await {
        return back("disabled");
    }
    if require_perm(&state, &headers, "agents.manage")
        .await
        .is_err()
    {
        return back("forbidden");
    }

    let qp = query_pairs(uri.query());
    if qp.get("error").is_some_and(|e| !e.is_empty()) {
        return back("denied");
    }
    let code = qp.get("code").cloned().filter(|s| !s.is_empty());
    let state_param = qp.get("state").cloned().filter(|s| !s.is_empty());
    let cookie_state = talaria_session::parse_cookies(&headers)
        .and_then(|c| c.get(STATE_COOKIE).cloned())
        .filter(|s| !s.is_empty());
    let (Some(code), Some(state_param), Some(cookie_state)) = (code, state_param, cookie_state)
    else {
        return back("bad_state");
    };
    if !state_matches(&state_param, &cookie_state) {
        return back("bad_state");
    }
    let agent_model = match take_agent_oauth_state(&state.pg, &state_param).await {
        Ok(Some(model)) => model,
        Ok(None) => return back("bad_state"),
        Err(e) => return internal("[integrations/google/agent] oauth state read failed", e),
    };

    let public_url = talaria_auth_config::get_auth_config().public_url;
    let redirect_uri = google_agent_connect_redirect_uri(public_url.as_deref(), &headers, &uri);
    let ex = match exchange_google_tokens(&state.pg, &sb, &code, &redirect_uri).await {
        Ok(ex) => ex,
        Err(e) => {
            tracing::error!("[integrations/google/agent] connect failed: {e}");
            return back("exchange_failed");
        }
    };
    // Kind first: a save that fails leaves the agent refusing writes instead
    // of silently keeping the org account the admin just replaced.
    if let Err(e) = upsert_principal(&state.pg, &agent_model, PrincipalKind::Agent, None).await {
        return internal("[integrations/google/agent] principal write failed", e);
    }
    let scope = ex
        .scope
        .clone()
        .unwrap_or_else(|| WORKSPACE_SCOPES.join(" "));
    if let Err(e) = save_agent_connection(
        &state.pg,
        &sb,
        &agent_model,
        &SaveAgentConnection {
            google_sub: &ex.sub,
            email: ex.email.as_deref(),
            scope: &scope,
            refresh_token: ex.refresh_token.as_deref(),
            access_token: ex.access_token.as_deref(),
            expires_in_seconds: ex.expires_in,
            connected_by: None,
            now_ms: now_ms(),
        },
    )
    .await
    {
        tracing::error!("[integrations/google/agent] connection save failed: {e}");
        return back("exchange_failed");
    }
    back("connected")
}

// /api/mcp/oauth/start.
// GET ?server=<id>&scope=org|me → 302 into the provider's authorization page.
// scope=org (one shared connection) needs agents.manage; scope=me connects
// the signed-in user's own account on a per-user server.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use talaria_api_facades::mcp::oauth::start_oauth;
use talaria_api_facades::mcp::registry::get_mcp_server;
use talaria_error::{house_error, internal};
use talaria_instance::instance_base_url;
use talaria_session::require_user;
use talaria_session::secretbox_or_500;
use talaria_state::AppState;

#[derive(Deserialize)]
pub struct StartQuery {
    server: Option<String>,
    scope: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<StartQuery>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let server_id = query.server.unwrap_or_default();
    let scope = if query.scope.as_deref() == Some("me") {
        "me"
    } else {
        "org"
    };
    let server = match get_mcp_server(&state.pg, &server_id).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[mcp/oauth] server read failed", e)),
    };
    let Some(server) = server else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    if scope == "org"
        && !talaria_users::has_perm(&state.pg, &user.id, &user.role, "agents.manage")
            .await
            .unwrap_or(false)
    {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let sb = secretbox_or_500(&state, "[mcp/oauth] secretbox unavailable").await?;
    // A verified hosting domain gives every OAuth app ONE stable callback
    // URL, whatever origin the admin happens to browse from.
    let origin = talaria_api_facades::google::oauth::resolve_origin(
        talaria_auth_config::get_auth_config().public_url.as_deref(),
        &headers,
        &uri,
    );
    let base = instance_base_url(&state.pg).await.unwrap_or(origin);
    Ok(
        match start_oauth(
            &state.pg,
            &sb,
            &server.id,
            &server.url,
            if scope == "me" { &user.id } else { "org" },
            &base,
        )
        .await
        {
            Ok(authorize) => (
                StatusCode::FOUND,
                [(header::LOCATION, authorize)],
                axum::body::Body::empty(),
            )
                .into_response(),
            Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
        },
    )
}

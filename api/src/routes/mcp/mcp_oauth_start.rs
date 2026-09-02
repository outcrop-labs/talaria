// /api/mcp/oauth/start — port of ui/src/routes/api/mcp.oauth.start.ts.
// GET ?server=<id>&scope=org|me → 302 into the provider's authorization page.
// scope=org (one shared connection) needs agents.manage; scope=me connects
// the signed-in user's own account on a per-user server.

use crate::error::house_error;
use crate::instance::instance_base_url;
use crate::mcp::oauth::start_oauth;
use crate::mcp::registry::get_mcp_server;
use crate::session::require_user;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

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
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let server_id = query.server.unwrap_or_default();
    let scope = if query.scope.as_deref() == Some("me") {
        "me"
    } else {
        "org"
    };
    let server = match get_mcp_server(&state.pg, &server_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[mcp/oauth] server read failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    let Some(server) = server else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    if scope == "org"
        && !crate::users::has_perm(&state.pg, &user.id, &user.role, "agents.manage")
            .await
            .unwrap_or(false)
    {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[mcp/oauth] secretbox unavailable: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    // A verified hosting domain gives every OAuth app ONE stable callback
    // URL, whatever origin the admin happens to browse from.
    let origin = crate::google::oauth::resolve_origin(
        crate::auth_config::get_auth_config().public_url.as_deref(),
        &headers,
        &uri,
    );
    let base = instance_base_url(&state.pg).await.unwrap_or(origin);
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
    }
}

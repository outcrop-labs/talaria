// /api/mcp/oauth/start.
// GET ?server=<id>&scope=org|me → 302 into the provider's authorization page.
// scope=org (one shared connection) needs agents.manage; scope=me connects
// the signed-in user's own account on a per-user server.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use talaria_api_facades::mcp::oauth::start_oauth;
use talaria_api_facades::mcp::registry::{McpServer, get_mcp_server};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_session::secretbox_or_500;
use talaria_state::AppState;

/// Why a `scope=me` start is refused on this server, or None when the server
/// takes a personal connection. Pure, so the rule is pinned without a stack.
fn me_scope_refusal(server: &McpServer) -> Option<&'static str> {
    if !server.enabled {
        Some("this server is disabled")
    } else if server.builtin || server.app_slug.is_some() {
        Some("this server has no account to connect")
    } else if server.auth_mode != "per-user" {
        Some("this server uses the workspace's connection, not a personal one")
    } else {
        None
    }
}

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
    // A personal connection only makes sense on a server that takes one:
    // enabled, per-user, and a real upstream (the builtin toolkit and an
    // app's in-process server have no account to connect). Anything else
    // is refused before this route discovers or registers anything against
    // the server's URL on the caller's say-so.
    if scope == "me"
        && let Some(why) = me_scope_refusal(&server)
    {
        return Ok(house_error(StatusCode::FORBIDDEN, why));
    }
    // A person denied the server (their own row, or a team deny with no
    // allow) cannot connect an account to it either — the connection would
    // be a credential the gateway never lets their assistant present.
    if scope == "me"
        && talaria_api_facades::mcp::registry::owner_denied(&state.pg, &server.id, &user.id)
            .await
            .map_err(|e| internal("[mcp/oauth] allowance read failed", e))?
    {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "your access to this server is denied, so there is no account to connect",
        ));
    }
    let sb = secretbox_or_500(&state, "[mcp/oauth] secretbox unavailable").await?;
    // A verified hosting domain gives every OAuth app ONE stable callback
    // URL, whatever origin the admin happens to browse from — the same base
    // the server card shows and the manual-client save stores.
    let base = super::oauth_callback_base(&state, &headers, &uri).await;
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

// DELETE /api/mcp/servers/{id}/oauth. Disconnect the org's OAuth account
// from one MCP server — admin only. The sealed org tokens are dropped, the
// gateway answers for the server in-house again (its not-connected handshake
// and the cached catalog, every call answered with the sentence that says an
// admin connects it), and the change APPLIES like the
// connect it undoes: the carriers re-list the server's tools on the gateway's
// list-changed notice at their next keepalive, and nothing restarts unless the
// render moved a container for some other reason. A per-user account is its
// owner's to disconnect (PUT /api/me/mcp with `headers: null`); this route
// touches only the org's. Audited as `mcp.oauth_disconnect`, with the admin as
// the actor.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::mcp::apply::{apply_agents_for_server, bump_tool_lists_for_ids};
use talaria_api_facades::mcp::oauth::{drop_oauth_tokens, has_oauth_tokens};
use talaria_api_facades::mcp::registry::get_mcp_server;
use talaria_audit::{AuditEntry, log_audit};
use talaria_error::{house_error, internal};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

/// The OAuth subject the org's one shared connection is stored under.
const ORG: &str = "org";

/// The refusal when there is nothing to disconnect — the em dash is part of
/// the wire contract.
const NOT_CONNECTED: &str = "this server has no org account connected — nothing to disconnect";

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    // Server-side, whatever the UI shows: an org connection is the
    // workspace's credential, and dropping it takes every carrier's tools.
    let user = require_admin(&state, &headers).await?;
    let server = match get_mcp_server(&state.pg, &id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[mcp/oauth] server read failed", e)),
    };
    match has_oauth_tokens(&state.pg, &server.id, ORG).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::CONFLICT, NOT_CONNECTED)),
        Err(e) => return Ok(internal("[mcp/oauth] token read failed", e)),
    }
    let sb = secretbox_or_500(&state, "[mcp/oauth] secretbox unavailable").await?;
    if let Err(e) = drop_oauth_tokens(&state.pg, &server.id, ORG).await {
        return Ok(internal("[mcp/oauth] org token drop failed", e));
    }
    // A disconnect on purpose settles a refused grant: the card should read
    // Connect next time, not Reconnect.
    talaria_api_facades::mcp::health::clear_reconnect(&state.pg, &server.id, ORG).await;
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "mcp.oauth_disconnect",
            target_type: "mcp-server",
            target_id: Some(&server.id),
            target_label: Some(&server.name),
            before: None,
            after: Some(json!({ "subject": ORG })),
        },
    )
    .await;
    let (pg, server_id) = (state.pg.clone(), server.id.clone());
    tokio::spawn(async move {
        // The connect's strokes, undone the same way (mcp_oauth_callback):
        // the gateway stops relaying the moment the tokens are gone; each
        // carrier keeps the server name it holds and re-lists its tools on
        // the notice the count below arms — the in-house catalog, whose
        // calls answer that the connection is not set up.
        let _ = talaria_api_facades::fleet::render::render_fleet(&pg, &sb).await;
        bump_tool_lists_for_ids(&pg, std::slice::from_ref(&server_id)).await;
        apply_agents_for_server(&pg, &sb, &server_id).await;
    });
    Ok(Json(json!({ "ok": true, "disconnected": true })).into_response())
}

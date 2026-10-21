// The MCP registry, gateway, oauth pair, library, icons, test.
pub mod mcp;

use axum::http::{HeaderMap, Uri};
use talaria_state::AppState;

/// THE ONE CALLBACK URL this instance registers with OAuth providers, spelled
/// once: the verified hosting domain when Admin → Org has one, else the
/// origin the request arrived on. Three doors used to compute it three ways
/// (the start route this way, the manual-client save from the Origin header,
/// the UI from `window.location`), and a provider that received one spelling
/// and was then asked to redirect to another refused the whole flow.
pub(crate) async fn oauth_callback_base(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> String {
    let origin = talaria_api_facades::google::oauth::resolve_origin(
        talaria_auth_config::get_auth_config().public_url.as_deref(),
        headers,
        uri,
    );
    talaria_instance::instance_base_url(&state.pg)
        .await
        .unwrap_or(origin)
}

pub(crate) fn oauth_callback_url(base: &str) -> String {
    format!("{}/api/mcp/oauth/callback", base.trim_end_matches('/'))
}
pub mod mcp_calls;
pub mod mcp_gw_server;
pub mod mcp_health;
pub mod mcp_icon;
pub mod mcp_library;
pub mod mcp_oauth_callback;
pub mod mcp_oauth_start;
pub mod mcp_servers;
pub mod mcp_servers_id;
pub mod mcp_test;

// /api/mcp/library.
// GET → the MCP server catalog, served from the LOCAL MIRROR of the official
// registry (`talaria_mcp_library::catalog`): `?featured=1` is the editorial
// shelf, `?q=` searches title/domain/name/description with full text, and
// `tier=official|verified|community` (best tier to include) / `kind=hosted|
// package` / `limit=` narrow it. Both arms answer `{servers, catalog}` —
// `catalog` is the mirror's status (`count`, `syncedAt`, `syncing`,
// `phase: empty|syncing|ready`, `error`) so the picker can say "syncing" or
// "no match" and mean it. Backs the Add-server picker and the agent MCP tab.
// POST /api/mcp/library/sync → start a sync now (the first-boot nudge and the
// picker's "Sync now"); answers `{ok, catalog}` immediately — the walk runs
// behind it.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::mcp::library::catalog::{self, SearchFilters};
use talaria_api_facades::mcp::library::{LibraryServer, Tier, library};
use talaria_session::require_perm;
use talaria_state::AppState;

#[derive(serde::Deserialize)]
pub struct LibraryQuery {
    q: Option<String>,
    featured: Option<String>,
    tier: Option<String>,
    kind: Option<String>,
    limit: Option<i64>,
}

/// The picker's wire shape — the struct's own serialization (camelCase, in
/// declaration order), which is also exactly what the mirror stores.
fn server_wire(s: &LibraryServer) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let lib = library();
    let status = catalog::status(&state.pg).await;
    // An empty mirror on a running instance means the first sync has not
    // happened yet (fresh install, or the job never got its lease) — nudge it
    // from here rather than wait for the next tick. Rate-limited by the
    // catalog's own attempt stamp, so an unreachable registry is one try
    // per window rather than one per page load.
    if status.nudgeable {
        catalog::trigger_sync(state.pg.clone(), lib.clone());
    }
    let servers = if query.featured.as_deref() == Some("1") {
        catalog::featured(&state.pg, &lib).await
    } else {
        let filters = SearchFilters {
            max_tier: query.tier.as_deref().and_then(Tier::parse),
            kind: query
                .kind
                .as_deref()
                .filter(|k| matches!(*k, "hosted" | "package"))
                .map(str::to_string),
            limit: query.limit,
        };
        // absent and empty are the same query.
        catalog::search(&state.pg, &lib, query.q.as_deref().unwrap_or(""), &filters).await
    };
    Ok(Json(json!({
        "servers": servers.iter().map(server_wire).collect::<Vec<_>>(),
        "catalog": catalog::status_wire(&status),
    }))
    .into_response())
}

pub async fn sync(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    catalog::trigger_sync(state.pg.clone(), library());
    let status = catalog::status(&state.pg).await;
    Ok(Json(json!({ "ok": true, "catalog": catalog::status_wire(&status) })).into_response())
}

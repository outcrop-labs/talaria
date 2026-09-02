// /api/mcp/library.
// GET ?q= → the MCP server library (the official registry, live, filtered to
// remote-capable servers). Backs the Add-server picker. BOTH arms answer
// `{servers}` — featured shelf and search alike; the picker indexes
// `data.servers`, never a bare array.

use crate::mcp::library::{LibraryServer, library};
use crate::session::require_perm;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

// No 502 arm here: a registry that won't answer surfaces as an empty shelf,
// not a thrown error — refresh only replaces a cached shelf with a NON-empty
// one — so there is no failure path to catch.

#[derive(serde::Deserialize)]
pub struct LibraryQuery {
    q: Option<String>,
    featured: Option<String>,
}

fn server_wire(s: &LibraryServer) -> Value {
    json!({
        "registryName": s.registry_name,
        "title": s.title,
        "description": s.description,
        "url": s.url,
        "domain": s.domain,
        "icon": s.icon,
        "tier": s.tier,
        "requiredHeaders": s.required_headers.iter().map(|h| json!({
            "name": h.name,
            "description": h.description,
            "isRequired": h.is_required,
            "isSecret": h.is_secret,
            "placeholder": h.placeholder,
            "default": h.default,
            "choices": h.choices,
        })).collect::<Vec<_>>(),
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let lib = library();
    if query.featured.as_deref() == Some("1") {
        let shelf = lib.featured().await;
        return Json(json!({ "servers": shelf.iter().map(server_wire).collect::<Vec<_>>() }))
            .into_response();
    }
    // absent and empty are the same query.
    let q = query.q.unwrap_or_default();
    let servers = lib.search(&q).await;
    Json(json!({ "servers": servers.iter().map(server_wire).collect::<Vec<_>>() })).into_response()
}

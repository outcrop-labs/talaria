// /api/mcp/library.
// GET ?q= → the MCP server library (the official registry, live, filtered to
// remote-capable servers). Backs the Add-server picker. BOTH arms answer
// `{servers}` — featured shelf and search alike; the picker indexes
// `data.servers`, never a bare array.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::mcp::library::{LibraryServer, library};
use talaria_session::require_perm;
use talaria_state::AppState;

// No 502 arm here: a registry that won't answer surfaces as an empty shelf,
// not a thrown error — refresh only replaces a cached shelf with a NON-empty
// one — so there is no failure path to catch.

#[derive(serde::Deserialize)]
pub struct LibraryQuery {
    q: Option<String>,
    featured: Option<String>,
}

fn server_wire(s: &LibraryServer) -> Value {
    let declared = |list: &Vec<talaria_api_facades::mcp::library::LibraryHeader>| {
        list.iter()
            .map(|h| {
                let variables = h.variables.as_ref().map(|m| {
                    let mut out = serde_json::Map::new();
                    for (k, v) in m {
                        out.insert(
                            k.clone(),
                            json!({
                                "description": v.description,
                                "isSecret": v.is_secret,
                                "placeholder": v.placeholder,
                                "default": v.default,
                                "choices": v.choices,
                            }),
                        );
                    }
                    Value::Object(out)
                });
                json!({
                    "name": h.name,
                    "description": h.description,
                    "isRequired": h.is_required,
                    "isSecret": h.is_secret,
                    "placeholder": h.placeholder,
                    "default": h.default,
                    "choices": h.choices,
                    "value": h.value,
                    "variables": variables,
                })
            })
            .collect::<Vec<_>>()
    };
    let package = s.package.as_ref().map(|p| {
        json!({
            "kind": p.kind,
            "identifier": p.identifier,
            "version": p.version,
            "runtimeHint": p.runtime_hint,
            "transport": p.transport,
            "containerPort": p.container_port,
            "transportPath": p.transport_path,
            "image": p.image,
            "runArgs": p.run_args,
            "declaredEnv": declared(&p.declared_env),
        })
    });
    json!({
        "registryName": s.registry_name,
        "title": s.title,
        "description": s.description,
        "url": s.url,
        "domain": s.domain,
        "icon": s.icon,
        "tier": s.tier,
        "requiredHeaders": declared(&s.required_headers),
        "package": package,
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let lib = library();
    if query.featured.as_deref() == Some("1") {
        let shelf = lib.featured().await;
        return Ok(
            Json(json!({ "servers": shelf.iter().map(server_wire).collect::<Vec<_>>() }))
                .into_response(),
        );
    }
    // absent and empty are the same query.
    let q = query.q.unwrap_or_default();
    let servers = lib.search(&q).await;
    Ok(
        Json(json!({ "servers": servers.iter().map(server_wire).collect::<Vec<_>>() }))
            .into_response(),
    )
}

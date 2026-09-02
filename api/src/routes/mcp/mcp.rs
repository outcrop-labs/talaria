// /api/mcp.
// GET → MCP servers per agent: the agent's own config version PLUS the org
// registry's assignments (rendered in at deploy — marked 'managed' here so
// the agent UI reflects what the /mcp view attached). Non-admins get the
// roster + server NAMES only — internal URLs and tool filters map the
// fleet's private attack surface, so they stay admin-only.

use crate::agent_mcp::{AgentMcp, list_agent_mcp};
use crate::error::thrown_internal_error;
use crate::mcp::registry::servers_for_agent;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

fn agent_wire(a: &AgentMcp) -> Value {
    json!({
        "id": a.id,
        "slug": a.slug,
        "displayName": a.display_name,
        "managed": a.managed,
        "servers": a.servers.iter().map(|s| json!({
            "name": s.name,
            "url": s.url,
            "timeout": s.timeout,
            "extras": s.extras,
        })).collect::<Vec<_>>(),
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let mut agents = match list_agent_mcp(&state.pg).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[mcp] roster failed: {e}");
            return thrown_internal_error();
        }
    };
    let models: Vec<(String, String)> =
        match sqlx::query_as("select id::text, model from agent_defs where enabled")
            .fetch_all(&state.pg)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("[mcp] roster models failed: {e}");
                return thrown_internal_error();
            }
        };
    for a in &mut agents {
        let Some(model) = models
            .iter()
            .find(|(id, _)| id == &a.id)
            .map(|(_, m)| m.clone())
        else {
            continue;
        };
        let registry = match servers_for_agent(&state.pg, &model).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[mcp] serversForAgent failed: {e}");
                return thrown_internal_error();
            }
        };
        for srv in registry {
            if a.servers.iter().any(|s| s.name == srv.name) {
                continue;
            }
            let url = format!("gateway://{}", srv.name);
            a.servers.push(crate::agent_mcp::McpServerEntry {
                name: srv.name,
                url,
                timeout: srv.timeout_secs,
                extras: vec!["managed".into()],
            });
        }
    }
    if user.role == "admin" {
        return Json(json!({ "agents": agents.iter().map(agent_wire).collect::<Vec<_>>() }))
            .into_response();
    }
    // Names only — and of the extras, just the two the UI renders.
    Json(json!({
        "agents": agents.iter().map(|a| {
            json!({
                "id": a.id,
                "slug": a.slug,
                "displayName": a.display_name,
                "managed": a.managed,
                "servers": a.servers.iter().map(|s| json!({
                    "name": s.name,
                    "url": "",
                    "timeout": Value::Null,
                    "extras": s.extras.iter().filter(|e| e.as_str() == "built-in" || e.as_str() == "managed").cloned().collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>()
    }))
    .into_response()
}

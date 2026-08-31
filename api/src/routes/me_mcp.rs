// /api/me/mcp — port of ui/src/routes/api/me.mcp.ts.
// Connected accounts (Settings → Connections): per-user MCP servers and
// whether YOU have connected yours. PUT { serverId, headers } connects
// (headers sealed at rest — e.g. { Authorization: "Bearer <your token>" });
// headers null disconnects. Your assistant only carries a per-user server
// once you've connected, and it acts as YOU there.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, record_msg, string_msg, too_big_msg, uuid_member, zod_type_name};
use crate::error::{house_error, thrown_internal_error};
use crate::mcp_apply::roll_agent_for_user;
use crate::mcp_oauth::{drop_oauth_tokens, has_oauth_tokens};
use crate::mcp_registry::{has_user_credentials, list_mcp_servers, set_user_credentials};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let servers = match list_mcp_servers(&state.pg).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[me/mcp] registry read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut out = Vec::new();
    for s in servers
        .iter()
        .filter(|s| s.enabled && s.auth_mode == "per-user")
    {
        let connected = if s.oauth_enabled {
            match has_oauth_tokens(&state.pg, &s.id, &user.id).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("[me/mcp] oauth read failed: {e}");
                    return thrown_internal_error();
                }
            }
        } else {
            match has_user_credentials(&state.pg, &s.id, &user.id).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("[me/mcp] credentials read failed: {e}");
                    return thrown_internal_error();
                }
            }
        };
        out.push(json!({
            "id": s.id,
            "name": s.name,
            "label": s.label,
            "description": s.description,
            "requiredHeaders": s.required_headers,
            "authKind": if s.oauth_enabled { "oauth" } else { "headers" },
            "connected": connected,
        }));
    }
    Json(json!({ "servers": out })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let server_id = match uuid_member(obj, "serverId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // `z.record(z.string(), z.string().max(4000)).nullable()` — null
    // disconnects; the values are the user's own credentials for the server.
    let creds: Option<serde_json::Map<String, Value>> = match obj.get("headers") {
        Some(Value::Null) => None,
        None => {
            return house_error(StatusCode::BAD_REQUEST, &string_msg("undefined"))
        }
        Some(Value::Object(m)) => {
            for v in m.values() {
                let Some(s) = v.as_str() else {
                    return house_error(
                        StatusCode::BAD_REQUEST,
                        &string_msg(zod_type_name(v)),
                    );
                };
                if crate::body::utf16_len(s) > 4000 {
                    return house_error(StatusCode::BAD_REQUEST, &too_big_msg(4000));
                }
            }
            Some(m.clone())
        }
        Some(other) => {
            return house_error(StatusCode::BAD_REQUEST, &record_msg(zod_type_name(other)))
        }
    };

    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[me/mcp] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    if let Err(e) = set_user_credentials(&state.pg, &sb, &server_id, &user.id, creds.as_ref()).await {
        tracing::error!("[me/mcp] credential store failed: {e}");
        return thrown_internal_error();
    }
    if creds.is_none()
        && let Err(e) = drop_oauth_tokens(&state.pg, &server_id, &user.id).await
    {
        tracing::error!("[me/mcp] oauth drop failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: if creds.is_some() { "mcp.connect" } else { "mcp.disconnect" },
            target_type: "mcp-server",
            target_id: Some(&server_id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    let (pg, sb_) = (state.pg.clone(), sb.clone());
    let user_id = user.id.clone();
    tokio::spawn(async move {
        // Config truth first…
        let _ = crate::fleet_render::render_fleet(&pg, &sb_, None).await;
        // …then the live cutover.
        roll_agent_for_user(&pg, &sb_, &user_id).await;
    });
    Json(json!({ "ok": true })).into_response()
}

// /api/me/mcp. Connected accounts (Settings → Connections): per-user MCP
// servers and whether YOU have connected yours. PUT { serverId, headers }
// connects (headers sealed at rest — e.g. { Authorization: "Bearer <your
// token>" }); headers null disconnects. Your assistant only carries a
// per-user server once you've connected, and it acts as YOU there.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::mcp::apply::roll_agent_for_user;
use talaria_api_facades::mcp::oauth::{drop_oauth_tokens, has_oauth_tokens};
use talaria_api_facades::mcp::registry::{
    has_user_credentials, list_mcp_servers, set_user_credentials,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    as_object, parse, record_msg, string_msg, too_big_msg, uuid_member, zod_type_name,
};
use talaria_error::{house_error, internal};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let servers = match list_mcp_servers(&state.pg).await {
        Ok(s) => s,
        Err(e) => return internal("[me/mcp] registry read failed", e),
    };
    let mut out = Vec::new();
    for s in servers
        .iter()
        .filter(|s| s.enabled && s.auth_mode == "per-user")
    {
        let connected = if s.oauth_enabled {
            match has_oauth_tokens(&state.pg, &s.id, &user.id).await {
                Ok(b) => b,
                Err(e) => return internal("[me/mcp] oauth read failed", e),
            }
        } else {
            match has_user_credentials(&state.pg, &s.id, &user.id).await {
                Ok(b) => b,
                Err(e) => return internal("[me/mcp] credentials read failed", e),
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
    // headers — a string→string record (values ≤4000) or null to disconnect;
    // the values are the user's own credentials for the server.
    let creds: Option<serde_json::Map<String, Value>> = match obj.get("headers") {
        Some(Value::Null) => None,
        None => return house_error(StatusCode::BAD_REQUEST, &string_msg("undefined")),
        Some(Value::Object(m)) => {
            for v in m.values() {
                let Some(s) = v.as_str() else {
                    return house_error(StatusCode::BAD_REQUEST, &string_msg(zod_type_name(v)));
                };
                if talaria_body::utf16_len(s) > 4000 {
                    return house_error(StatusCode::BAD_REQUEST, &too_big_msg(4000));
                }
            }
            Some(m.clone())
        }
        Some(other) => {
            return house_error(StatusCode::BAD_REQUEST, &record_msg(zod_type_name(other)));
        }
    };

    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => return internal("[me/mcp] secretbox unavailable", e),
    };
    if let Err(e) = set_user_credentials(&state.pg, &sb, &server_id, &user.id, creds.as_ref()).await
    {
        return internal("[me/mcp] credential store failed", e);
    }
    if creds.is_none()
        && let Err(e) = drop_oauth_tokens(&state.pg, &server_id, &user.id).await
    {
        return internal("[me/mcp] oauth drop failed", e);
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: if creds.is_some() {
                "mcp.connect"
            } else {
                "mcp.disconnect"
            },
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
        let _ = talaria_api_facades::fleet::render::render_fleet(&pg, &sb_, None).await;
        // …then the live cutover.
        roll_agent_for_user(&pg, &sb_, &user_id).await;
    });
    Json(json!({ "ok": true })).into_response()
}

// /api/me/mcp. Connected accounts (Settings → Connections): per-user MCP
// servers and whether YOU have connected yours. PUT { serverId, headers }
// connects (headers sealed at rest — e.g. { Authorization: "Bearer <your
// token>" }); headers null disconnects. Your assistant only carries a
// per-user server once you've connected, and it acts as YOU there.
// `needsReconnect` marks an OAuth account the provider refused to refresh
// (found by the MCP health pass) — connected once, now gone.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::mcp::apply::{apply_agent_for_user, bump_tool_lists};
use talaria_api_facades::mcp::oauth::{drop_oauth_tokens, has_oauth_tokens};
use talaria_api_facades::mcp::registry::{
    get_mcp_server, has_user_credentials, list_mcp_servers, set_user_credentials,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, record_msg, string_msg, too_big_msg, uuid_member, zod_type_name};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user, secretbox_or_500};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let servers = match list_mcp_servers(&state.pg).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[me/mcp] registry read failed", e)),
    };
    // Grants the provider refused to refresh (the health pass flags them):
    // the row says Reconnect, not a first-time Connect.
    let reconnect = talaria_api_facades::mcp::health::reconnect_servers(&state.pg, &user.id).await;
    let mut out = Vec::new();
    for s in servers
        .iter()
        .filter(|s| s.enabled && s.auth_mode == "per-user")
    {
        let connected = if s.oauth_enabled {
            match has_oauth_tokens(&state.pg, &s.id, &user.id).await {
                Ok(b) => b,
                Err(e) => return Ok(internal("[me/mcp] oauth read failed", e)),
            }
        } else {
            match has_user_credentials(&state.pg, &s.id, &user.id).await {
                Ok(b) => b,
                Err(e) => return Ok(internal("[me/mcp] credentials read failed", e)),
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
            "needsReconnect": !connected && reconnect.contains(&s.id),
        }));
    }
    Ok(Json(json!({ "servers": out })).into_response())
}

/// Names a connected account may never set, whatever the server declares:
/// the framing and hop-by-hop headers (the relay owns those), the upstream
/// session id, and cookies. `authorization` is the one credential name that
/// is allowed WITHOUT a declaration — a server declaring nothing takes a
/// bearer, which is what the connect form offers — but it is refused when
/// the server declares a different set.
const NEVER_HEADER_NAMES: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
    "proxy-connection",
    "proxy-authorization",
    "mcp-session-id",
    "cookie",
];

/// Why this header name is refused for a personal connection, or None when
/// it may be stored. `declared` is the server's `requiredHeaders` names;
/// when it names any, only those names are taken (case-insensitively), so a
/// person connects what the card asked for and nothing else. Pure, so the
/// rule is pinned without a stack.
fn header_name_refusal(name: &str, declared: &[String]) -> Option<String> {
    let lower = name.trim().to_lowercase();
    if lower.is_empty() || axum::http::HeaderName::from_bytes(lower.as_bytes()).is_err() {
        return Some(format!("\"{name}\" is not a valid header name"));
    }
    if NEVER_HEADER_NAMES.contains(&lower.as_str()) {
        return Some(format!(
            "the \"{name}\" header cannot be set by a connected account"
        ));
    }
    if declared.is_empty() {
        return None;
    }
    if declared
        .iter()
        .any(|d| d.trim().eq_ignore_ascii_case(&lower))
    {
        None
    } else {
        Some(format!(
            "the \"{name}\" header is not one this server asks for ({})",
            declared.join(", ")
        ))
    }
}

/// The header names a server declares, from its `requiredHeaders` entries.
fn declared_header_names(required_headers: &Value) -> Vec<String> {
    required_headers
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|h| h.get("name").and_then(Value::as_str))
                .filter(|n| !n.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let server_id = match uuid_member(obj, "serverId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // headers — a string→string record (values ≤4000) or null to disconnect;
    // the values are the user's own credentials for the server.
    let creds: Option<serde_json::Map<String, Value>> = match obj.get("headers") {
        Some(Value::Null) => None,
        None => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &string_msg("undefined"),
            ));
        }
        Some(Value::Object(m)) => {
            for v in m.values() {
                let Some(s) = v.as_str() else {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &string_msg(zod_type_name(v)),
                    ));
                };
                if talaria_body::utf16_len(s) > 4000 {
                    return Ok(house_error(StatusCode::BAD_REQUEST, &too_big_msg(4000)));
                }
            }
            Some(m.clone())
        }
        Some(other) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &record_msg(zod_type_name(other)),
            ));
        }
    };

    // The server the credentials are for: it has to exist, and a WRITE only
    // lands on a server that takes a personal connection — enabled and
    // per-user — and only under the header names it declares. A disconnect
    // is allowed on any existing server, so a person can always clear a
    // credential they stored.
    let server = match get_mcp_server(&state.pg, &server_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[me/mcp] server read failed", e)),
    };
    if let Some(m) = &creds {
        if !server.enabled || server.auth_mode != "per-user" {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "this server does not take a personal connection",
            ));
        }
        let declared = declared_header_names(&server.required_headers);
        for name in m.keys() {
            if let Some(why) = header_name_refusal(name, &declared) {
                return Ok(house_error(StatusCode::BAD_REQUEST, &why));
            }
        }
    }

    let sb = secretbox_or_500(&state, "[me/mcp] secretbox unavailable").await?;
    if let Err(e) = set_user_credentials(&state.pg, &sb, &server_id, &user.id, creds.as_ref()).await
    {
        return Ok(internal("[me/mcp] credential store failed", e));
    }
    if creds.is_none()
        && let Err(e) = drop_oauth_tokens(&state.pg, &server_id, &user.id).await
    {
        return Ok(internal("[me/mcp] oauth drop failed", e));
    }
    // Any write settles a refused grant: a disconnect on purpose should read
    // Connect next time, not Reconnect.
    talaria_api_facades::mcp::health::clear_reconnect(&state.pg, &server_id, &user.id).await;
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
        // The gateway speaks for the person's account from this moment (or
        // stops, on a disconnect); their assistant keeps the server it holds
        // and re-lists its tools on the gateway's list-changed notice at its
        // next keepalive. The apply renders, and restarts the assistant only
        // if the render moved its container (talaria-mcp-apply).
        let _ = talaria_api_facades::fleet::render::render_fleet(&pg, &sb_).await;
        bump_tool_lists(&pg).await;
        apply_agent_for_user(&pg, &sb_, &user_id).await;
    });
    Ok(Json(json!({ "ok": true })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_and_framing_names_are_always_refused() {
        let none: Vec<String> = Vec::new();
        for name in [
            "host",
            "Host",
            "content-length",
            "Transfer-Encoding",
            "connection",
            "mcp-session-id",
            "Cookie",
            "proxy-authorization",
        ] {
            assert!(
                header_name_refusal(name, &none).is_some(),
                "{name} must be refused even when nothing is declared"
            );
            assert!(
                header_name_refusal(name, &[name.to_string()]).is_some(),
                "{name} must be refused even when declared"
            );
        }
        // Not a header name at all.
        assert!(header_name_refusal("", &none).is_some());
        assert!(header_name_refusal("x y", &none).is_some());
        assert!(header_name_refusal("bad\nname", &none).is_some());
    }

    #[test]
    fn a_declared_set_admits_exactly_those_names_and_authorization_stands_in_for_none() {
        let none: Vec<String> = Vec::new();
        // Nothing declared: a bearer (the connect form's default) and any
        // ordinary credential header are taken.
        assert_eq!(header_name_refusal("Authorization", &none), None);
        assert_eq!(header_name_refusal("X-Api-Key", &none), None);
        // Declared: only the declared names, case-insensitively.
        let declared = vec!["X-Api-Key".to_string(), "X-Org".to_string()];
        assert_eq!(header_name_refusal("x-api-key", &declared), None);
        assert_eq!(header_name_refusal("X-ORG", &declared), None);
        let why = header_name_refusal("Authorization", &declared).expect("undeclared");
        assert!(why.contains("X-Api-Key, X-Org"), "{why}");
        assert!(header_name_refusal("X-Other", &declared).is_some());
        // Declared names come off the registry's entry shape.
        assert_eq!(
            declared_header_names(
                &json!([{ "name": "X-Api-Key" }, { "name": " " }, { "nope": 1 }])
            ),
            vec!["X-Api-Key".to_string()]
        );
        assert!(declared_header_names(&json!(null)).is_empty());
    }
}

// /api/mcp/servers — port of ui/src/routes/api/mcp.servers.ts.
// The org MCP registry. GET → servers + their assignments + user access
// (admin/agents.manage view). POST → register a server. Every mutation
// re-renders the fleet so configs pick the change up (Hermes re-reads on
// mtime — no restarts).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, array_msg, array_too_big_msg, as_object, nullable_number_member, nullish_max_string_member,
    object_msg, optional_boolean_member, optional_enum_member, optional_max_string_member, parse,
    record_msg, string_msg, too_big_msg, url_member, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::mcp_oauth::{ensure_oauth_config, has_oauth_tokens, oauth_meta};
use crate::mcp_registry::{
    NewServer, create_mcp_server, get_mcp_server, list_assignments, list_mcp_servers,
    list_user_access, server_wire,
};
use crate::secretbox::SecretBox;
use crate::session::{actor_of, require_perm};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sqlx::PgPool;

/// `z.record(z.string(), z.string().max(2000)).optional()` — any keys, string
/// values bounded to 2000. Secrets live here, so the response never carries
/// them back: GET answers every key masked.
fn optional_headers_member(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<Option<Map<String, Value>>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let Value::Object(m) = v else {
        return Err(record_msg(zod_type_name(v)));
    };
    for v in m.values() {
        let Some(s) = v.as_str() else {
            return Err(string_msg(zod_type_name(v)));
        };
        if crate::body::utf16_len(s) > 2000 {
            return Err(too_big_msg(2000));
        }
    }
    Ok(Some(m.clone()))
}

/// `z.string().regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase slug').max(60)`:
/// type, then the pattern (zod's custom message), then length.
fn slug_member(obj: &Map<String, Value>, key: &str) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| string_msg("undefined"))?;
    let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
    let mut chars = s.chars();
    let head_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = s
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !(head_ok && rest_ok) {
        return Err("lowercase slug".into());
    }
    if utf16_len(s) > 60 {
        return Err(too_big_msg(60));
    }
    Ok(s.to_string())
}

/// `z.number().int().positive().max(n).nullish()` — the int guard runs in
/// the shared reader (with its own safe-int bounds); the exclusive `>0` and
/// the cap are zod 4's own sentences.
fn nullish_positive_int_member(
    obj: &Map<String, Value>,
    key: &str,
    max: f64,
) -> Result<Option<i64>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    // Bounds here never trip (the int guard already capped the range); the
    // real checks follow in zod's order.
    let n = nullable_number_member(obj, key, NumKind::Int, f64::MIN, f64::INFINITY)?
        .ok_or("unreachable: present non-null value read above")?;
    if n <= 0.0 {
        return Err("Too small: expected number to be >0".into());
    }
    if n > max {
        return Err(format!("Too big: expected number to be <={}", max as i64));
    }
    Ok(Some(n as i64))
}

/// One element of the `requiredHeaders` array — header declarations captured
/// at install to drive per-user connect forms.
struct DeclaredHeader {
    name: Value,
    description: Option<String>,
    is_secret: Value,
    placeholder: Option<String>,
}

fn required_headers_member(
    obj: &Map<String, Value>,
) -> Result<Option<Vec<DeclaredHeader>>, String> {
    let Some(v) = obj.get("requiredHeaders") else {
        return Ok(None);
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::new();
    for el in arr {
        let m = el.as_object().ok_or_else(|| object_msg(zod_type_name(el)))?;
        let name = match m.get("name") {
            Some(Value::String(s)) if utf16_len(s) <= 120 => Value::String(s.clone()),
            Some(Value::String(_)) => return Err(too_big_msg(120)),
            Some(other) => return Err(string_msg(zod_type_name(other))),
            None => return Err(string_msg("undefined")),
        };
        let description = nullish_max_string_member(m, "description", 500)?;
        let is_secret = optional_boolean_member(m, "isSecret")?;
        let placeholder = nullish_max_string_member(m, "placeholder", 200)?;
        out.push(DeclaredHeader {
            name,
            description,
            is_secret: is_secret.map(Value::Bool).unwrap_or(Value::Null),
            placeholder,
        });
    }
    if arr.len() > 10 {
        return Err(array_too_big_msg(10));
    }
    Ok(Some(out))
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let servers = match list_mcp_servers(&state.pg).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[mcp] registry read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut detail = Vec::with_capacity(servers.len());
    for s in &servers {
        let mut wire = server_wire(s);
        // Never echo secrets — every stored key answers masked.
        if let Some(Value::Object(h)) = wire.get_mut("headers") {
            let keys: Vec<String> = h.keys().cloned().collect();
            for k in keys {
                h.insert(k, json!("•••"));
            }
        }
        let assignments = match list_assignments(&state.pg, &s.id).await {
            Ok(rows) => rows
                .into_iter()
                .map(|(agent_model, tools)| {
                    json!({ "agentModel": agent_model, "tools": tools })
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::error!("[mcp] assignments read failed: {e}");
                return thrown_internal_error();
            }
        };
        let user_access = match list_user_access(&state.pg, &s.id).await {
            Ok(rows) => rows
                .into_iter()
                .map(|(user_id, allowed, tools)| {
                    json!({ "userId": user_id, "allowed": allowed, "tools": tools })
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::error!("[mcp] user access read failed: {e}");
                return thrown_internal_error();
            }
        };
        let org_connected = if s.oauth_enabled {
            match has_oauth_tokens(&state.pg, &s.id, "org").await {
                Ok(b) => json!(b),
                Err(e) => {
                    tracing::error!("[mcp] oauth token read failed: {e}");
                    return thrown_internal_error();
                }
            }
        } else {
            json!(null)
        };
        let oauth_meta_v = if s.oauth_enabled {
            match oauth_meta(&state.pg, &s.id).await {
                Ok(m) => m.unwrap_or(Value::Null),
                Err(e) => {
                    tracing::error!("[mcp] oauth meta read failed: {e}");
                    return thrown_internal_error();
                }
            }
        } else {
            Value::Null
        };
        if let Some(o) = wire.as_object_mut() {
            o.insert("assignments".into(), Value::Array(assignments));
            o.insert("userAccess".into(), Value::Array(user_access));
            o.insert("orgConnected".into(), org_connected);
            o.insert("oauthMeta".into(), oauth_meta_v);
        }
        detail.push(wire);
    }
    Json(json!({ "servers": detail })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return crate::error::house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The zod body, checked in schema order.
    let name = match slug_member(obj, "name") {
        Ok(v) => v,
        Err(msg) => return crate::error::house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let label = match optional_max_string_member(obj, "label", 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match nullish_max_string_member(obj, "description", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let url = match url_member(obj, "url", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let headers_in = match optional_headers_member(obj, "headers") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let timeout_secs = match nullish_positive_int_member(obj, "timeoutSecs", 3600.0) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let auth_mode = match optional_enum_member(obj, "authMode", &["org", "per-user"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let declared = match required_headers_member(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // Everything from create through the response construction sits in TS's
    // one try — any failure is the route's 400, with the duplicate-name
    // special case.
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => return bad_request(&format!("secretbox unavailable: {e}")),
    };
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let outcome = create_and_sniff(
        &state.pg,
        &NewServer {
            name: &name,
            label: label.as_deref(),
            description: description.as_deref(),
            url: &url,
            headers: headers_in.as_ref(),
            timeout_secs,
            auth_mode: auth_mode.as_deref().unwrap_or("org"),
            required_headers: &declared_json(&declared),
            created_by: &created_by,
        },
    )
    .await;
    let server = match outcome {
        Ok(s) => s,
        Err(e) => return bad_request(&e),
    };
    spawn_audit_and_render(&state.pg, &sb, &user, &server);
    let meta = match oauth_meta(&state.pg, &server.id).await {
        Ok(m) => m,
        Err(e) => return bad_request(&e.to_string()),
    };
    let mut wire = server_wire(&server);
    wire.as_object_mut()
        .expect("server_wire is an object")
        .insert("oauthMeta".into(), meta.unwrap_or(Value::Null));
    Json(json!({ "server": wire })).into_response()
}

/// The try-block's core: create, sniff the auth shape, re-read when the
/// sniff flipped OAuth on.
async fn create_and_sniff(
    pg: &PgPool,
    input: &NewServer<'_>,
) -> Result<crate::mcp_registry::McpServer, String> {
    let mut server = create_mcp_server(pg, input).await?;
    // Sniff the auth shape right away: a 401 challenge with resource
    // metadata marks the server OAuth and unlocks the Connect flow.
    if ensure_oauth_config(pg, &server.id, &server.url)
        .await?
        .is_some()
        && let Some(fresh) = get_mcp_server(pg, &server.id).await.map_err(|e| e.to_string())?
    {
        server = fresh;
    }
    Ok(server)
}

fn declared_json(declared: &Option<Vec<DeclaredHeader>>) -> Value {
    Value::Array(
        declared
            .as_ref()
            .map(|rows| {
                rows.iter()
                    .map(|h| {
                        json!({
                            "name": h.name,
                            "description": h.description,
                            "isSecret": h.is_secret,
                            "placeholder": h.placeholder,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn bad_request(message: &str) -> Response {
    // A unique-violation lands in sqlx's message as "duplicate key value
    // violates unique constraint" — TS matches on 'duplicate'.
    let error = if message.contains("duplicate") {
        "that name is taken".to_string()
    } else {
        message.to_string()
    };
    crate::error::house_error(StatusCode::BAD_REQUEST, &error)
}

fn spawn_audit_and_render(
    pg: &PgPool,
    sb: &SecretBox,
    user: &crate::session::SessionUser,
    server: &crate::mcp_registry::McpServer,
) {
    let pg = pg.clone();
    let sb = sb.clone();
    let actor = actor_of(user);
    let audit_server_id = server.id.clone();
    let audit_name = server.name.clone();
    let audit_url = server.url.clone();
    let audit_auth_mode = server.auth_mode.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "mcp.server_add",
                target_type: "mcp-server",
                target_id: Some(&audit_server_id),
                target_label: Some(&audit_name),
                before: None,
                after: Some(json!({ "url": audit_url, "authMode": audit_auth_mode })),
            },
        )
        .await;
        let _ = crate::fleet_render::render_fleet(&pg, &sb, None).await;
    });
}

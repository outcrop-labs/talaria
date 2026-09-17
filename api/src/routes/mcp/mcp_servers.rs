// /api/mcp/servers.
// The org MCP registry. GET → servers + their assignments + user/team access
// (admin/agents.manage view). POST → register a server. Every mutation
// re-renders the fleet so configs pick the change up (Hermes re-reads on
// mtime — no restarts).

use std::collections::HashMap;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, array_msg, array_too_big_msg, as_object, nullable_number_member,
    nullish_max_string_member, object_msg, optional_enum_member, optional_max_string_member, parse,
    record_msg, string_msg, too_big_msg, url_member, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::mcp::oauth::{ensure_oauth_config, has_oauth_tokens, oauth_meta};
use crate::mcp::registry::{
    NewServer, create_mcp_server, get_mcp_server, list_assignments, list_mcp_servers,
    list_team_access, list_user_access, server_wire,
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
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
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
/// at install to drive per-user connect forms. Carries the registry's
/// `InputWithVariables` shape: a fixed `value` (optionally `{templated}`)
/// with a `variables` map describing each fill-in.
struct DeclaredHeader {
    name: Value,
    description: Option<String>,
    is_required: Value,
    is_secret: Value,
    placeholder: Option<String>,
    default: Option<String>,
    choices: Option<Vec<String>>,
    value: Option<String>,
    variables: Option<HashMap<String, DeclaredVariable>>,
}

/// One `variables` entry — an Input, sharing the prompt metadata minus the
/// header-only fields.
#[derive(Debug, PartialEq)]
struct DeclaredVariable {
    description: Option<String>,
    is_secret: Value,
    placeholder: Option<String>,
    default: Option<String>,
    choices: Option<Vec<String>>,
}

/// `z.array(z.string().max(200)).max(20).nullish()`.
fn choices_member(m: &Map<String, Value>) -> Result<Option<Vec<String>>, String> {
    let Some(v) = m.get("choices") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::new();
    for el in arr {
        let Value::String(s) = el else {
            return Err(string_msg(zod_type_name(el)));
        };
        if utf16_len(s) > 200 {
            return Err(too_big_msg(200));
        }
        out.push(s.clone());
    }
    if arr.len() > 20 {
        return Err(array_too_big_msg(20));
    }
    Ok(Some(out))
}

/// A boolean that may also be null — declared_json writes null for an absent
/// flag, so a round-tripped row must parse. Wrong types still error.
fn nullish_boolean_member(m: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| {
            format!(
                "Invalid input: expected boolean, received {}",
                zod_type_name(v)
            )
        }),
    }
}

fn declared_variable(m: &Map<String, Value>) -> Result<DeclaredVariable, String> {
    Ok(DeclaredVariable {
        description: nullish_max_string_member(m, "description", 500)?,
        is_secret: nullish_boolean_member(m, "isSecret")?
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        placeholder: nullish_max_string_member(m, "placeholder", 200)?,
        default: nullish_max_string_member(m, "default", 200)?,
        choices: choices_member(m)?,
    })
}

fn declared_variables_member(
    m: &Map<String, Value>,
) -> Result<Option<HashMap<String, DeclaredVariable>>, String> {
    let Some(v) = m.get("variables") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let obj = v.as_object().ok_or_else(|| record_msg(zod_type_name(v)))?;
    if obj.len() > 10 {
        return Err("Too big: expected record to have <=10 entries".into());
    }
    let mut out = HashMap::with_capacity(obj.len());
    for (k, el) in obj {
        let m = el
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(el)))?;
        out.insert(k.clone(), declared_variable(m)?);
    }
    Ok(Some(out))
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
        let m = el
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(el)))?;
        let name = match m.get("name") {
            Some(Value::String(s)) if utf16_len(s) <= 120 => Value::String(s.clone()),
            Some(Value::String(_)) => return Err(too_big_msg(120)),
            Some(other) => return Err(string_msg(zod_type_name(other))),
            None => return Err(string_msg("undefined")),
        };
        let description = nullish_max_string_member(m, "description", 500)?;
        let is_required = nullish_boolean_member(m, "isRequired")?;
        let is_secret = nullish_boolean_member(m, "isSecret")?;
        let placeholder = nullish_max_string_member(m, "placeholder", 200)?;
        let default = nullish_max_string_member(m, "default", 200)?;
        let choices = choices_member(m)?;
        let value = nullish_max_string_member(m, "value", 2000)?;
        let variables = declared_variables_member(m)?;
        out.push(DeclaredHeader {
            name,
            description,
            is_required: is_required.map(Value::Bool).unwrap_or(Value::Null),
            is_secret: is_secret.map(Value::Bool).unwrap_or(Value::Null),
            placeholder,
            default,
            choices,
            value,
            variables,
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
                .map(|(agent_model, tools)| json!({ "agentModel": agent_model, "tools": tools }))
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
        let team_access = match list_team_access(&state.pg, &s.id).await {
            Ok(rows) => rows
                .into_iter()
                .map(|(team_id, allowed, tools)| {
                    json!({ "teamId": team_id, "allowed": allowed, "tools": tools })
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::error!("[mcp] team access read failed: {e}");
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
            o.insert("teamAccess".into(), Value::Array(team_access));
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

    // Everything from create through the response construction is one
    // unbroken stretch — any failure is the route's 400, with the
    // duplicate-name special case.
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

/// Create, sniff the auth shape, re-read when the sniff flipped OAuth on.
async fn create_and_sniff(
    pg: &PgPool,
    input: &NewServer<'_>,
) -> Result<crate::mcp::registry::McpServer, String> {
    let mut server = create_mcp_server(pg, input).await?;
    // Sniff the auth shape right away: a 401 challenge with resource
    // metadata marks the server OAuth and unlocks the Connect flow.
    if ensure_oauth_config(pg, &server.id, &server.url)
        .await?
        .is_some()
        && let Some(fresh) = get_mcp_server(pg, &server.id)
            .await
            .map_err(|e| e.to_string())?
    {
        server = fresh;
    }
    Ok(server)
}

fn declared_json(declared: &Option<Vec<DeclaredHeader>>) -> Value {
    fn variable_json(v: &DeclaredVariable) -> Value {
        json!({
            "description": v.description,
            "isSecret": v.is_secret,
            "placeholder": v.placeholder,
            "default": v.default,
            "choices": v.choices,
        })
    }
    Value::Array(
        declared
            .as_ref()
            .map(|rows| {
                rows.iter()
                    .map(|h| {
                        json!({
                            "name": h.name,
                            "description": h.description,
                            "isRequired": h.is_required,
                            "isSecret": h.is_secret,
                            "placeholder": h.placeholder,
                            "default": h.default,
                            "choices": h.choices,
                            "value": h.value,
                            "variables": h.variables.as_ref().map(|m| {
                                Value::Object(
                                    m.iter()
                                        .map(|(k, v)| (k.clone(), variable_json(v)))
                                        .collect(),
                                )
                            }),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn bad_request(message: &str) -> Response {
    // A unique-violation lands in sqlx's message as "duplicate key value
    // violates unique constraint" — the 'duplicate' substring is the detector.
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
    server: &crate::mcp::registry::McpServer,
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
        let _ = crate::fleet::render::render_fleet(&pg, &sb, None).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn member(body: Value) -> Result<Option<Vec<DeclaredHeader>>, String> {
        required_headers_member(body.as_object().expect("test bodies are objects"))
    }

    #[test]
    fn parses_the_full_input_with_variables_shape() {
        let parsed = member(json!({ "requiredHeaders": [{
            "name": "Authorization",
            "description": "Bearer token",
            "isRequired": true,
            "isSecret": true,
            "value": "Bearer {api_key}",
            "variables": {
                "api_key": { "placeholder": "sk-…", "isSecret": true, "default": "x" }
            },
        }]}))
        .expect("the registry shape parses");
        let rows = parsed.expect("present");
        assert_eq!(rows.len(), 1);
        let h = &rows[0];
        assert_eq!(h.value.as_deref(), Some("Bearer {api_key}"));
        assert_eq!(h.is_required, Value::Bool(true));
        let vars = h.variables.as_ref().expect("variables kept");
        let v = vars.get("api_key").expect("the entry parses");
        assert_eq!(v.placeholder.as_deref(), Some("sk-…"));
        assert_eq!(v.is_secret, Value::Bool(true));
        assert_eq!(v.default.as_deref(), Some("x"));
    }

    #[test]
    fn an_absent_value_keeps_yesterdays_stored_shape() {
        // Old clients and old rows: name/description/isSecret/placeholder only.
        let parsed = member(json!({ "requiredHeaders": [{
            "name": "X-Key",
            "description": "the key",
            "isSecret": null,
            "placeholder": "abc",
        }]}))
        .expect("the old shape still parses");
        let rows = parsed.expect("present");
        let h = &rows[0];
        assert_eq!(h.value, None);
        assert_eq!(h.variables, None);
        assert_eq!(h.is_required, Value::Null);
        // And it round-trips through declared_json with nulls, never gaps.
        let wire = declared_json(&Some(rows));
        assert_eq!(
            wire[0]["value"],
            Value::Null,
            "absent fields serialize null so the wire shape never shifts"
        );
    }

    #[test]
    fn caps_reject_oversized_values_choices_and_variables() {
        let long_value = "v".repeat(2001);
        assert!(
            member(json!({ "requiredHeaders": [{ "name": "X", "value": long_value }]})).is_err()
        );
        let many_choices: Vec<String> = (0..21).map(|i| i.to_string()).collect();
        assert!(
            member(json!({ "requiredHeaders": [{ "name": "X", "choices": many_choices }]}))
                .is_err()
        );
        let mut vars = serde_json::Map::new();
        for i in 0..11 {
            vars.insert(format!("v{i}"), json!({}));
        }
        assert!(member(json!({ "requiredHeaders": [{ "name": "X", "variables": vars }]})).is_err());
        // A non-string value is a type error, not a silent drop.
        assert!(member(json!({ "requiredHeaders": [{ "name": "X", "value": 7 }]})).is_err());
    }
}

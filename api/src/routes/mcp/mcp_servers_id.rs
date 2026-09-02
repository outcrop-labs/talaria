// /api/mcp/servers/{id}.
// One registry server: PUT patches config / assignment / user access / tool
// refresh in one idempotent surface; DELETE unregisters (assignments, user
// access, and connected accounts cascade). Fleet re-renders after mutations.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    array_msg, as_object, optional_boolean_member, optional_enum_member,
    optional_max_string_member, optional_string_array_member, optional_url_member, parse,
    string_msg, uuid_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::mcp::apply::{
    carriers_for_server, enqueue_rolls, roll_agent_for_model, roll_agent_for_user,
    roll_agents_for_server,
};
use crate::mcp::oauth::{ensure_oauth_config, set_manual_oauth_client};
use crate::mcp::registry::{
    ServerPatch, delete_mcp_server, get_mcp_server, refresh_mcp_tools, remove_assignment,
    set_assignment, set_user_access, update_mcp_server,
};
use crate::session::{actor_of, require_perm};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let server = match get_mcp_server(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[mcp] server read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(server) = server else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let actor = actor_of(&user);
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[mcp] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };

    // Self-heal: failed/aged discovery re-probes and backfills on any touch.
    if let Err(e) = ensure_oauth_config(&state.pg, &server.id, &server.url).await {
        // a self-heal throw is the route's 500, not a 400.
        tracing::error!("[mcp] oauth self-heal failed: {e}");
        return thrown_internal_error();
    }

    let patch = match parse_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Some(oauth_client) = &patch.oauth_client {
        // `{origin}/api/mcp/oauth/callback` — the callback this instance
        // registers with the provider. Callers come through the frontend
        // hop, so the request's ORIGIN HEADER is the browser-facing origin —
        // not this API's own.
        let origin = headers
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .unwrap_or_else(|| String::from("http://127.0.0.1:5274"));
        if let Err(e) = set_manual_oauth_client(
            &state.pg,
            &sb,
            &server.id,
            &server.url,
            &oauth_client.client_id,
            oauth_client.client_secret.as_deref(),
            &format!("{origin}/api/mcp/oauth/callback"),
        )
        .await
        {
            return house_error(StatusCode::BAD_REQUEST, &e);
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "mcp.oauth_client",
                target_type: "mcp-server",
                target_id: Some(&server.id),
                target_label: Some(&server.name),
                before: None,
                after: None,
            },
        )
        .await;
    }

    let mut audit_after = Map::new(); // `{...config, headers: keys?}` in schema order
    let config_touched = patch.label.is_some()
        || patch.description.is_some()
        || patch.url.is_some()
        || patch.headers.is_some()
        || patch.timeout_secs.is_some()
        || patch.enabled.is_some()
        || patch.all_agents.is_some()
        || patch.auth_mode.is_some();
    if config_touched {
        // An omitted headers field keeps the stored secrets; sending {} clears.
        if let Err(e) = update_mcp_server(
            &state.pg,
            &server.id,
            &ServerPatch {
                label: patch.label.clone(),
                description: patch.description.clone(),
                url: patch.url.clone(),
                headers: patch.headers.clone(),
                timeout_secs: patch.timeout_secs,
                enabled: patch.enabled,
                all_agents: patch.all_agents,
                auth_mode: patch.auth_mode.clone(),
            },
        )
        .await
        {
            // A guard's refusal and a DB failure alike are the route's 500 —
            // nothing catches this call.
            tracing::error!("[mcp] server update failed: {e}");
            return thrown_internal_error();
        }
        if let Some(label) = &patch.label {
            audit_after.insert("label".into(), json!(label));
        }
        if let Some(description) = &patch.description {
            audit_after.insert(
                "description".into(),
                description
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        if let Some(url) = &patch.url {
            audit_after.insert("url".into(), json!(url));
        }
        if let Some(h) = &patch.headers {
            audit_after.insert(
                "headers".into(),
                Value::Array(h.keys().map(|k| Value::String(k.clone())).collect()),
            );
        }
        if let Some(timeout) = &patch.timeout_secs {
            audit_after.insert(
                "timeoutSecs".into(),
                timeout
                    .map(|t| Value::Number(crate::body::js_num(t as f64)))
                    .unwrap_or(Value::Null),
            );
        }
        if let Some(enabled) = patch.enabled {
            audit_after.insert("enabled".into(), json!(enabled));
        }
        if let Some(all_agents) = patch.all_agents {
            audit_after.insert("allAgents".into(), json!(all_agents));
        }
        if let Some(auth_mode) = &patch.auth_mode {
            audit_after.insert("authMode".into(), json!(auth_mode));
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "mcp.server_update",
                target_type: "mcp-server",
                target_id: Some(&server.id),
                target_label: Some(&server.name),
                before: None,
                after: Some(Value::Object(audit_after)),
            },
        )
        .await;
    }
    if let Some(assign) = &patch.assign {
        if let Err(e) = set_assignment(
            &state.pg,
            &server.id,
            &assign.agent_model,
            assign.tools.as_deref(),
        )
        .await
        {
            tracing::error!("[mcp] assign failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "mcp.assign",
                target_type: "mcp-server",
                target_id: Some(&server.id),
                target_label: Some(&server.name),
                before: None,
                after: Some(json!({ "agentModel": assign.agent_model, "tools": assign.tools })),
            },
        )
        .await;
    }
    if let Some(unassign) = &patch.unassign {
        if let Err(e) = remove_assignment(&state.pg, &server.id, unassign).await {
            tracing::error!("[mcp] unassign failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "mcp.unassign",
                target_type: "mcp-server",
                target_id: Some(&server.id),
                target_label: Some(&server.name),
                before: None,
                after: Some(json!({ "agentModel": unassign })),
            },
        )
        .await;
    }
    if let Some(user_access) = &patch.user_access {
        if let Err(e) = set_user_access(
            &state.pg,
            &server.id,
            &user_access.user_id,
            user_access.allowed,
            user_access.tools.as_deref(),
        )
        .await
        {
            tracing::error!("[mcp] user access failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "mcp.user_access",
                target_type: "mcp-server",
                target_id: Some(&server.id),
                target_label: Some(&server.name),
                before: None,
                after: Some(json!({
                    "userId": user_access.user_id,
                    "allowed": user_access.allowed,
                    "tools": user_access.tools,
                })),
            },
        )
        .await;
    }
    let mut tools: Option<Vec<Value>> = None;
    if patch.refresh_tools {
        match refresh_mcp_tools(&state.pg, &sb, &server.id).await {
            Ok(list) => tools = Some(list),
            Err(e) => {
                return house_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("tool discovery failed: {e}"),
                );
            }
        }
    }
    spawn_render(&state.pg, &sb);
    // Live cutover for the agents this change touches: a running Hermes
    // only wires MCP servers at start, so carriers roll blue/green. Rolls
    // are fire-and-forget — never awaited, never the caller's problem.
    let touched_access =
        patch.enabled.is_some() || patch.all_agents.is_some() || patch.auth_mode.is_some();
    if touched_access {
        let (pg, sb_, id) = (state.pg.clone(), sb.clone(), server.id.clone());
        spawn_roll(async move {
            roll_agents_for_server(&pg, &sb_, &id).await;
        });
    } else if let Some(assign) = &patch.assign {
        let (pg, sb_, model) = (state.pg.clone(), sb.clone(), assign.agent_model.clone());
        spawn_roll(async move {
            roll_agent_for_model(&pg, &sb_, &model).await;
        });
    } else if let Some(unassign) = patch.unassign.clone() {
        let (pg, sb_) = (state.pg.clone(), sb.clone());
        spawn_roll(async move {
            roll_agent_for_model(&pg, &sb_, &unassign).await;
        });
    } else if let Some(user_access) = &patch.user_access {
        let (pg, sb_, user_id) = (state.pg.clone(), sb.clone(), user_access.user_id.clone());
        spawn_roll(async move {
            roll_agent_for_user(&pg, &sb_, &user_id).await;
        });
    }
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    if let Some(tools) = tools {
        out.insert("tools".into(), Value::Array(tools));
    }
    Json(Value::Object(out)).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let server = match get_mcp_server(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[mcp] server read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(server) = server else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    // Captured before the row vanishes.
    let carriers = match carriers_for_server(&state.pg, &server.id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[mcp] carriers read failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Err(e) = delete_mcp_server(&state.pg, &server.id).await {
        return house_error(StatusCode::BAD_REQUEST, &e);
    }
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[mcp] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    enqueue_rolls(&carriers, &state.pg, &sb);
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "mcp.server_delete",
            target_type: "mcp-server",
            target_id: Some(&server.id),
            target_label: Some(&server.name),
            before: None,
            after: None,
        },
    )
    .await;
    spawn_render(&state.pg, &sb);
    Json(json!({ "ok": true })).into_response()
}

fn spawn_render(pg: &sqlx::PgPool, sb: &crate::secretbox::SecretBox) {
    let pg = pg.clone();
    let sb = sb.clone();
    tokio::spawn(async move {
        let _ = crate::fleet::render::render_fleet(&pg, &sb, None).await;
    });
}

/// Fire-and-forget: the roll future resolves (or dies) on its own.
fn spawn_roll(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    tokio::spawn(async move {
        let _ = fut.await;
    });
}

// ── The zod patch shape ─────────────────────────────────────────────────────

struct AssignPatch {
    agent_model: String,
    tools: Option<Vec<String>>,
}

struct UserAccessPatch {
    user_id: String,
    allowed: Option<bool>,
    tools: Option<Vec<String>>,
}

struct OauthClientPatch {
    client_id: String,
    client_secret: Option<String>,
}

struct Patch {
    label: Option<String>,
    description: Option<Option<String>>,
    url: Option<String>,
    headers: Option<Map<String, Value>>,
    timeout_secs: Option<Option<i64>>,
    enabled: Option<bool>,
    all_agents: Option<bool>,
    auth_mode: Option<String>,
    refresh_tools: bool,
    assign: Option<AssignPatch>,
    unassign: Option<String>,
    user_access: Option<UserAccessPatch>,
    oauth_client: Option<OauthClientPatch>,
}

/// `z.record(z.string(), z.string().max(2000)).optional()`.
fn optional_headers_member(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<Option<Map<String, Value>>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let Value::Object(m) = v else {
        return Err(crate::body::record_msg(zod_type_name(v)));
    };
    for v in m.values() {
        let Some(s) = v.as_str() else {
            return Err(string_msg(zod_type_name(v)));
        };
        if crate::body::utf16_len(s) > 2000 {
            return Err(crate::body::too_big_msg(2000));
        }
    }
    Ok(Some(m.clone()))
}

/// `z.object({agentModel, tools: z.array(z.string().max(120)).nullable()})`
/// — a null tools list means "every tool".
fn assign_member(obj: &Map<String, Value>, key: &str) -> Result<Option<AssignPatch>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let m = v
        .as_object()
        .ok_or_else(|| crate::body::object_msg(zod_type_name(v)))?;
    let agent_model = crate::body::string_member(m, "agentModel", 1, 200)?;
    // `z.array(z.string().max(120)).nullable()` — null means "every tool",
    // an array is the subset.
    let tools = match m.get("tools") {
        None => Err(array_msg("undefined")),
        Some(Value::Null) => Ok(None),
        Some(_) => optional_string_array_member(m, "tools", 0, 120, u32::MAX as usize),
    }?;
    Ok(Some(AssignPatch { agent_model, tools }))
}

/// `z.object({userId: Uuid, allowed: z.boolean().nullable(), tools})`.
fn user_access_member(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<Option<UserAccessPatch>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let m = v
        .as_object()
        .ok_or_else(|| crate::body::object_msg(zod_type_name(v)))?;
    let user_id = uuid_member(m, "userId")?;
    let allowed = match m.get("allowed") {
        None => Err(crate::body::boolean_msg("undefined")),
        Some(Value::Null) => Ok(None),
        Some(_) => crate::body::boolean_member(m, "allowed").map(Some),
    }?;
    let tools = match m.get("tools") {
        None => Err(array_msg("undefined")),
        Some(Value::Null) => Ok(None),
        Some(_) => optional_string_array_member(m, "tools", 0, 120, u32::MAX as usize),
    }?;
    Ok(Some(UserAccessPatch {
        user_id,
        allowed,
        tools,
    }))
}

/// `z.object({clientId: z.string().min(1).max(200), clientSecret:
/// z.string().max(500).nullable()})` — pre-registered OAuth app credentials
/// for providers without dynamic registration.
fn oauth_client_member(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<Option<OauthClientPatch>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let m = v
        .as_object()
        .ok_or_else(|| crate::body::object_msg(zod_type_name(v)))?;
    let client_id = crate::body::string_member(m, "clientId", 1, 200)?;
    let client_secret = crate::body::nullable_optional_string_member(m, "clientSecret", 500)?;
    Ok(Some(OauthClientPatch {
        client_id,
        client_secret,
    }))
}

fn parse_patch(obj: &Map<String, Value>) -> Result<Patch, String> {
    let label = optional_max_string_member(obj, "label", 120)?;
    // `z.string().max(500).nullish()` as a PATCH member is tri-state: absent
    // leaves the column, null clears it, a string sets it — so this is
    // Option<Option<_>>, unlike the focus panel's folding nullish helper.
    let description = match obj.get("description") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => optional_max_string_member(obj, "description", 500).map(Some),
    }?;
    let url = optional_url_member(obj, "url", 500)?;
    let headers = optional_headers_member(obj, "headers")?;
    // `z.number().int().positive().max(3600).nullish()` — tri-state.
    let timeout_secs = match obj.get("timeoutSecs") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(_) => {
            let n = crate::body::nullable_number_member(
                obj,
                "timeoutSecs",
                crate::body::NumKind::Int,
                f64::MIN,
                f64::INFINITY,
            )?
            .ok_or("unreachable: present non-null read above")?;
            if n <= 0.0 {
                return Err("Too small: expected number to be >0".into());
            }
            if n > 3600.0 {
                return Err(format!("Too big: expected number to be <={}", 3600i64));
            }
            Some(Some(n as i64))
        }
    };
    let enabled = optional_boolean_member(obj, "enabled")?;
    let all_agents = optional_boolean_member(obj, "allAgents")?;
    let auth_mode = optional_enum_member(obj, "authMode", &["org", "per-user"])?;
    let refresh_tools = optional_boolean_member(obj, "refreshTools")?.unwrap_or(false);
    let assign = assign_member(obj, "assign")?;
    let unassign = optional_string_member_max(obj, "unassign", 200)?;
    let user_access = user_access_member(obj, "userAccess")?;
    let oauth_client = oauth_client_member(obj, "oauthClient")?;
    Ok(Patch {
        label,
        description,
        url,
        headers,
        timeout_secs,
        enabled,
        all_agents,
        auth_mode,
        refresh_tools,
        assign,
        unassign,
        user_access,
        oauth_client,
    })
}

/// `z.string().min(1).max(200).optional()` — the unassign member.
fn optional_string_member_max(
    obj: &Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(_) => crate::body::string_member(obj, key, 1, max).map(Some),
    }
}

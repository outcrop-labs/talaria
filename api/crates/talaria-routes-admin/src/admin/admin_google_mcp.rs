// /api/admin/google/mcp. The Google Workspace MCP pack (Developer Preview):
// Google's own MCP servers for Gmail, Drive, Calendar, Docs, Sheets, Slides,
// Chat, People and Universal Search, behind the MCP gateway. Off by default
// and per instance: no product reaches any agent until an admin enables it
// here, and an enabled product's registry row (`google-<product>`,
// `auth_mode = 'google'`) reaches only the agents it is then assigned to on
// the MCP page. Each agent acts with its own Google principal (its owner's
// connection, the org account, or its own identity) through the one door, so
// a refresh token never leaves the server and an owner never falls back to
// the org account. Read-only: only the tools on the pinned read list that
// Google also marks read-only are relayed; every other tool is refused, and
// held for a person once that approval exists. Deliberately requireAdmin.
//   GET → every product: its endpoint and the two Google Cloud services to
//   enable, whether it is enabled here, its registry row and last probe, and
//   each cached tool with the pack's policy for it (allow, hold, deny — a
//   hold is refused in this release) and the scopes Google accepts for it;
//   plus Google's preview terms, and the last refusals Google's servers
//   gave each Google grant (`refusals`).
//   POST /{product} {acknowledgePreview: true} → enable it: the registry
//   row, assigned to no agent, and its catalog asked of Google. 409 when it
//   is already enabled; 400 without the acknowledgement.
//   DELETE /{product} → disable it: the row goes, with its assignments, and
//   the agents that carried it drop it on their next reconcile.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::google::mcp::{PRODUCTS, Product, pack, policy};
use talaria_api_facades::mcp::apply::{carriers_for_server, enqueue_applies};
use talaria_api_facades::mcp::google::AUTH_MODE;
use talaria_api_facades::mcp::registry::{
    McpServer, NewServer, create_mcp_server, delete_mcp_server, get_mcp_server, refresh_mcp_tools,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::parse;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

/// The pack row for `p`, when it is enabled: the registry row with the
/// pack's name AND auth mode (a server someone registered under that name by
/// hand is not the pack's).
async fn pack_row(state: &AppState, p: &Product) -> Result<Option<McpServer>, sqlx::Error> {
    Ok(get_mcp_server(&state.pg, &pack::server_name(p))
        .await?
        .filter(|s| s.auth_mode == AUTH_MODE))
}

/// One cached tool as the panel shows it.
fn tool_view(p: &Product, t: &Value) -> Option<Value> {
    let name = t.get("name").and_then(Value::as_str)?;
    let default = policy::default_policy(p.id, name, Some(t));
    let effective = policy::gateway_policy(p.id, name, &Value::Array(vec![t.clone()]));
    Some(json!({
        "name": name,
        "title": t.get("title").cloned().or_else(|| t.pointer("/annotations/title").cloned()),
        "readOnlyHint": t.pointer("/annotations/readOnlyHint").cloned(),
        "destructiveHint": t.pointer("/annotations/destructiveHint").cloned(),
        "pinnedRead": policy::pinned_read(p.id, name),
        "policy": default,
        "effective": effective,
        "requiredScopesAnyOf": t.get("requiredScopesAnyOf").cloned(),
    }))
}

/// One product as the panel shows it.
fn product_view(p: &Product, row: Option<&McpServer>) -> Value {
    let tools: Vec<Value> = row
        .and_then(|s| s.tools.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| tool_view(p, t))
        .collect();
    json!({
        "id": p.id,
        "name": p.name,
        "endpoint": p.endpoint,
        "mcpService": p.mcp_service,
        "restService": p.rest_service,
        "enabled": row.is_some(),
        "server": row.map(|s| json!({
            "id": s.id,
            "name": s.name,
            "label": s.label,
            "enabled": s.enabled,
            "allAgents": s.all_agents,
            "toolsRefreshedMs": s.tools_refreshed_ms,
            "probe": s.last_probe,
        })),
        "tools": tools,
    })
}

/// The last refusal Google's MCP servers gave each Google grant, newest
/// first: whose grant (a person's, the org account's, an agent's own), the
/// status, Google's words, the tool, and whether it flags the grant for a
/// reconnect (a 401 a fresh token did not cure, until Google accepts that
/// grant again).
async fn refusals(state: &AppState) -> Result<Vec<Value>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Value)> = sqlx::query_as(
        "select * from ( \
           select 'owner'::text as principal, coalesce(u.email, g.email) as who, g.last_mcp_refusal as refusal \
             from google_connections g left join users u on u.id = g.user_id \
            where g.last_mcp_refusal is not null \
           union all \
           select 'org'::text, o.email, o.last_mcp_refusal from google_org_connection o \
            where o.last_mcp_refusal is not null \
           union all \
           select 'agent'::text, a.agent_model, a.last_mcp_refusal from agent_google_connections a \
            where a.last_mcp_refusal is not null \
         ) r order by r.refusal->>'at' desc limit 20",
    )
    .fetch_all(&state.pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(principal, who, refusal)| {
            json!({
                "principal": principal,
                "who": who,
                "status": refusal.get("status").cloned(),
                "text": refusal.get("text").cloned(),
                "tool": refusal.get("tool").cloned(),
                "at": refusal.get("at").cloned(),
                "flagged": refusal.get("flagged").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let mut products = Vec::new();
    for p in PRODUCTS {
        let row = match pack_row(&state, &p).await {
            Ok(r) => r,
            Err(e) => return Ok(internal("[admin/google-mcp] registry read failed", e)),
        };
        products.push(product_view(&p, row.as_ref()));
    }
    let refusals = match refusals(&state).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[admin/google-mcp] refusal read failed", e)),
    };
    Ok(Json(json!({
        "previewTerms": pack::PREVIEW_TERMS,
        "previewTermsUrl": pack::PREVIEW_TERMS_URL,
        "holdsEnabled": policy::HOLDS_ENABLED,
        "products": products,
        "refusals": refusals,
    }))
    .into_response())
}

/// The 404 for a product the pack does not offer.
fn no_such_product() -> Response {
    house_error(StatusCode::NOT_FOUND, "no such Google Workspace MCP server")
}

pub async fn post(
    State(state): State<AppState>,
    Path(product): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let Some(p) = pack::product(&product) else {
        return Ok(no_such_product());
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    if obj.get("acknowledgePreview").and_then(Value::as_bool) != Some(true) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "Google's Workspace MCP servers are in Developer Preview, whose terms say \u{ab}{}\u{bb} ({}). Send acknowledgePreview: true to enable one.",
                pack::PREVIEW_TERMS,
                pack::PREVIEW_TERMS_URL
            ),
        ));
    }
    match pack_row(&state, &p).await {
        Ok(Some(_)) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                &format!("{} is already enabled", p.name),
            ));
        }
        Ok(None) => {}
        Err(e) => return Ok(internal("[admin/google-mcp] registry read failed", e)),
    }
    let sb = secretbox_or_500(&state, "[admin/google-mcp] secretbox unavailable").await?;
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let name = pack::server_name(&p);
    let label = pack::server_label(&p);
    let description = format!(
        "Google's {} MCP server (Developer Preview). Read-only tools only; each agent acts with its own Google account.",
        p.name
    );
    let endpoint = pack::endpoint(&p);
    let required = json!([]);
    let created = create_mcp_server(
        &state.pg,
        &sb,
        &NewServer {
            name: &name,
            label: Some(&label),
            description: Some(&description),
            url: &endpoint,
            headers: None,
            timeout_secs: None,
            auth_mode: AUTH_MODE,
            required_headers: &required,
            created_by: &created_by,
            package: None,
            env_enc: None,
        },
    )
    .await;
    let server = match created {
        Ok(s) => s,
        Err(e) if e.contains("duplicate key") => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                &format!(
                    "a server named {name} is already registered; remove it on the MCP page first"
                ),
            ));
        }
        Err(e) => return Ok(internal("[admin/google-mcp] create failed", e)),
    };
    // The catalog, asked of Google now: the policy reads it, and an agent
    // assigned before it is known would see no tools. Best-effort — the
    // row stands either way, and its probe says what Google answered.
    if let Err(e) = refresh_mcp_tools(&state.pg, &sb, &server.id).await {
        tracing::warn!("[admin/google-mcp] {name}: catalog refresh failed: {e}");
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.mcp_enable",
            target_type: "mcp-server",
            target_id: Some(&server.id),
            target_label: Some(&name),
            before: None,
            after: Some(json!({ "product": p.id, "acknowledgePreview": true })),
        },
    )
    .await;
    let row = match pack_row(&state, &p).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[admin/google-mcp] registry read failed", e)),
    };
    Ok(Json(json!({ "ok": true, "product": product_view(&p, row.as_ref()) })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(product): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let Some(p) = pack::product(&product) else {
        return Ok(no_such_product());
    };
    let row = match pack_row(&state, &p).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::NOT_FOUND,
                &format!("{} is not enabled", p.name),
            ));
        }
        Err(e) => return Ok(internal("[admin/google-mcp] registry read failed", e)),
    };
    let sb = secretbox_or_500(&state, "[admin/google-mcp] secretbox unavailable").await?;
    let carriers = match carriers_for_server(&state.pg, &row.id).await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[admin/google-mcp] carriers read failed", e)),
    };
    if let Err(e) = delete_mcp_server(&state.pg, &row.id).await {
        return Ok(internal("[admin/google-mcp] delete failed", e));
    }
    // A removed server is a name gone from its carriers' configs: an apply.
    enqueue_applies(&carriers, &state.pg, &sb);
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.mcp_disable",
            target_type: "mcp-server",
            target_id: Some(&row.id),
            target_label: Some(&row.name),
            before: None,
            after: Some(json!({ "product": p.id })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}

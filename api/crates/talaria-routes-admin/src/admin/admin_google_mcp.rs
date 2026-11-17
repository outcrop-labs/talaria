// /api/admin/google/mcp. The Google Workspace MCP pack (Developer Preview):
// Google's own MCP servers for Gmail, Drive, Calendar, Docs, Sheets, Slides,
// Chat, People and Universal Search, behind the MCP gateway. Off by default
// and per instance: no product reaches any agent until an admin enables it
// here, and an enabled product's registry row (`google-<product>`,
// `auth_mode = 'google'`) reaches only the agents it is then assigned to on
// the MCP page. Each agent acts with its own Google principal (its owner's
// connection, the org account, or its own identity) through the one door, so
// a refresh token never leaves the server and an owner never falls back to
// the org account. What runs is the pack's policy per tool: the tools on the
// pinned read list that Google also marks read-only are relayed; trash, spam
// and sensitive labels are denied; every other tool — every write, and any
// tool Google adds later — is HELD: the exact call waits in the approval
// queue for a person (`mcp_call`). An admin may override the default per
// tool, except that the deny list stays denied. Deliberately requireAdmin.
//   GET → every product: its endpoint and the two Google Cloud services to
//   enable, whether it is enabled here, its registry row and last probe, and
//   each cached tool with the pack's default policy for it (allow, hold,
//   deny), the admin's override if any, the policy in effect, and the scopes
//   Google accepts for it; plus Google's preview terms, and the last
//   refusals Google's servers gave each Google grant (`refusals`).
//   POST /{product} {acknowledgePreview: true} → enable it: the registry
//   row, assigned to no agent, and its catalog asked of Google. 409 when it
//   is already enabled; 400 without the acknowledgement.
//   DELETE /{product} → disable it: the row goes, with its assignments, its
//   tool overrides, and the agents that carried it drop it on their next
//   reconcile.
//   PUT /{product}/tools/{tool} {policy: "allow"|"hold"|"deny"|null} → set
//   (or, with null, clear) the admin's override for one tool the server's
//   catalog lists. 400 for a tool on the deny list set to anything but
//   deny; 404 for a product not enabled or a tool the catalog does not
//   list. Audited (`google.mcp_tool_policy`, before and after). The agents
//   holding the server are told to list its tools again.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::google::mcp::{PRODUCTS, Product, pack, policy};
use talaria_api_facades::mcp::apply::{bump_tool_lists, carriers_for_server, enqueue_applies};
use talaria_api_facades::mcp::google::{AUTH_MODE, tool_overrides};
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

/// One cached tool as the panel shows it: the pack's default, the admin's
/// override, and the policy in effect (the override, except that the deny
/// list stays denied).
fn tool_view(p: &Product, t: &Value, overrides: &[(String, policy::Policy)]) -> Option<Value> {
    let name = t.get("name").and_then(Value::as_str)?;
    let default = policy::default_policy(p.id, name, Some(t));
    let override_ = policy::override_of(overrides, name);
    let effective = policy::policy_for(p.id, name, Some(t), override_);
    Some(json!({
        "name": name,
        "title": t.get("title").cloned().or_else(|| t.pointer("/annotations/title").cloned()),
        "readOnlyHint": t.pointer("/annotations/readOnlyHint").cloned(),
        "destructiveHint": t.pointer("/annotations/destructiveHint").cloned(),
        "pinnedRead": policy::pinned_read(p.id, name),
        "denyListed": policy::denied_by_name(name),
        "policy": default,
        "override": override_,
        "effective": effective,
        "requiredScopesAnyOf": t.get("requiredScopesAnyOf").cloned(),
    }))
}

/// One product as the panel shows it.
fn product_view(
    p: &Product,
    row: Option<&McpServer>,
    overrides: &[(String, policy::Policy)],
) -> Value {
    let tools: Vec<Value> = row
        .and_then(|s| s.tools.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| tool_view(p, t, overrides))
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
        let overrides = match &row {
            Some(r) => match tool_overrides(&state.pg, &r.id).await {
                Ok(o) => o,
                Err(e) => return Ok(internal("[admin/google-mcp] policy read failed", e)),
            },
            None => Vec::new(),
        };
        products.push(product_view(&p, row.as_ref(), &overrides));
    }
    let refusals = match refusals(&state).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[admin/google-mcp] refusal read failed", e)),
    };
    Ok(Json(json!({
        "previewTerms": pack::PREVIEW_TERMS,
        "previewTermsUrl": pack::PREVIEW_TERMS_URL,
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
        "Google's {} MCP server (Developer Preview). Read-only tools run; every other call waits for a person to approve it. Each agent acts with its own Google account.",
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
    Ok(Json(json!({ "ok": true, "product": product_view(&p, row.as_ref(), &[]) })).into_response())
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

/// What a PUT asks for: a policy, or null to clear the override. Pure.
fn requested_policy(body: &Value) -> Result<Option<policy::Policy>, String> {
    let Some(obj) = body.as_object() else {
        return Err("body must be a JSON object".into());
    };
    match obj.get("policy") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => policy::Policy::parse(s)
            .map(Some)
            .ok_or_else(|| "policy must be \"allow\", \"hold\", \"deny\" or null".into()),
        _ => Err("policy must be \"allow\", \"hold\", \"deny\" or null".into()),
    }
}

pub async fn put_tool(
    State(state): State<AppState>,
    Path((product, tool)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let Some(p) = pack::product(&product) else {
        return Ok(no_such_product());
    };
    let wanted = match requested_policy(&parse(&body)) {
        Ok(w) => w,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(want) = wanted
        && let Some(why) = policy::override_refusal(&tool, want)
    {
        return Ok(house_error(StatusCode::BAD_REQUEST, why));
    }
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
    let Some(entry) = policy::catalog_entry(&row.tools, &tool) else {
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            &format!(
                "Google's {} MCP server did not list a tool named \u{201c}{tool}\u{201d} when Talaria last asked it",
                p.name
            ),
        ));
    };
    let before_overrides = match tool_overrides(&state.pg, &row.id).await {
        Ok(o) => o,
        Err(e) => return Ok(internal("[admin/google-mcp] policy read failed", e)),
    };
    let before = policy::override_of(&before_overrides, &tool);
    let written = match wanted {
        Some(want) => {
            sqlx::query(
                "insert into mcp_tool_policies (server_id, tool, policy, set_by, updated_at) \
             values ($1::uuid, $2, $3, $4::uuid, now()) \
             on conflict (server_id, tool) do update \
               set policy = excluded.policy, set_by = excluded.set_by, updated_at = now()",
            )
            .bind(&row.id)
            .bind(&tool)
            .bind(want.as_str())
            .bind(&user.id)
            .execute(&state.pg)
            .await
        }
        None => {
            sqlx::query("delete from mcp_tool_policies where server_id = $1::uuid and tool = $2")
                .bind(&row.id)
                .bind(&tool)
                .execute(&state.pg)
                .await
        }
    };
    if let Err(e) = written {
        return Ok(internal("[admin/google-mcp] policy write failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.mcp_tool_policy",
            target_type: "mcp-server",
            target_id: Some(&row.id),
            target_label: Some(&format!("{}/{tool}", row.name)),
            before: Some(json!({
                "override": before,
                "effective": policy::policy_for(p.id, &tool, Some(entry), before),
            })),
            after: Some(json!({
                "override": wanted,
                "effective": policy::policy_for(p.id, &tool, Some(entry), wanted),
            })),
        },
    )
    .await;
    // What an agent may list changed: those holding the server list again.
    bump_tool_lists(&state.pg, std::slice::from_ref(&row.name)).await;
    let overrides = match tool_overrides(&state.pg, &row.id).await {
        Ok(o) => o,
        Err(e) => return Ok(internal("[admin/google-mcp] policy read failed", e)),
    };
    Ok(Json(json!({ "ok": true, "tool": tool_view(&p, entry, &overrides) })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_put_names_a_policy_or_clears_one() {
        assert_eq!(
            requested_policy(&json!({ "policy": "allow" })),
            Ok(Some(policy::Policy::Allow))
        );
        assert_eq!(
            requested_policy(&json!({ "policy": "hold" })),
            Ok(Some(policy::Policy::Hold))
        );
        assert_eq!(
            requested_policy(&json!({ "policy": "deny" })),
            Ok(Some(policy::Policy::Deny))
        );
        assert_eq!(requested_policy(&json!({ "policy": null })), Ok(None));
        for bad in [
            json!({}),
            json!({ "policy": "Allow" }),
            json!({ "policy": 1 }),
            json!("allow"),
        ] {
            assert!(requested_policy(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_panel_shows_default_override_and_effect() {
        let p = pack::product("gmail").unwrap();
        let draft = json!({ "name": "create_draft", "annotations": { "readOnlyHint": false } });
        let trash = json!({ "name": "trash_thread", "annotations": { "destructiveHint": true } });
        let overrides = vec![
            ("create_draft".to_string(), policy::Policy::Allow),
            ("trash_thread".to_string(), policy::Policy::Allow),
        ];
        let v = tool_view(&p, &draft, &overrides).unwrap();
        assert_eq!(v["policy"], "hold");
        assert_eq!(v["override"], "allow");
        assert_eq!(v["effective"], "allow");
        assert_eq!(v["denyListed"], false);
        // A row on a deny-listed tool (never written by this route) changes
        // nothing: the effect stays deny.
        let v = tool_view(&p, &trash, &overrides).unwrap();
        assert_eq!(v["effective"], "deny");
        assert_eq!(v["denyListed"], true);
        let v = tool_view(&p, &draft, &[]).unwrap();
        assert_eq!(v["override"], Value::Null);
        assert_eq!(v["effective"], "hold");
    }
}

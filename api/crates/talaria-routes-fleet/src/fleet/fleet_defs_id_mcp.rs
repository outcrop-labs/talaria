// /api/fleet/defs/{id}/mcp. ONE AGENT'S VIEW OF THE ORG MCP REGISTRY.
// GET → every enabled registry server with what it means for THIS agent:
// whether the agent carries it — exactly what its rendered config lists
// (`servers_for_agent`) — and why (`via`: all-agents, assigned, team,
// developer — or null; `withheld` when a grant exists but the config leaves
// the server out), the per-agent tool subset if one is set, the server's
// own state (tools, auth, connection) so the tab can say what a toggle will
// actually give the agent — plus `legacy`: raw `mcp_servers` entries still in
// the agent's Hermes config from before the gateway, which bypass it and
// should go. `canInstall` says whether this viewer may add servers to the
// registry (`agents.manage`); assigning an existing one is the agent's
// managers' call. `assignedToolsMissing` names the subset entries the
// server no longer offers (the tab's "Drop missing tools"). POST → `assign`
// / `unassign` registry servers on this agent (a tool subset per
// assignment), `removeLegacy` raw entries; the agent rolls
// so a running Hermes picks the change up. Raw URL `add` is refused: an
// agent reaches servers through the gateway or not at all.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use talaria_agent_defs::{add_version_if_changed, list_versions};
use talaria_agent_mcp::apply_mcp_edits;
use talaria_api_facades::fleet::reconcile::roll_agent;
use talaria_api_facades::mcp::health::{missing_tools, tool_names};
use talaria_api_facades::mcp::oauth::has_oauth_tokens;
use talaria_api_facades::mcp::registry::{
    WORKBENCH_SERVER, get_mcp_server, list_mcp_servers, remove_assignment, servers_for_agent,
    set_assignment,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    array_msg, array_too_big_msg, optional_boolean_member, optional_string_array_member, parse,
    string_msg, too_big_msg, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_agent_manager, require_agent_reader, secretbox_or_500};
use talaria_state::AppState;

/// The def columns this route reads.
struct Def {
    id: String,
    slug: String,
    model: String,
    managed: bool,
    department: String,
    display_name: String,
    developer: bool,
    owner_user_id: Option<String>,
}

async fn load_def(pg: &sqlx::PgPool, id: &str) -> Result<Option<Def>, sqlx::Error> {
    let row: Option<(String, String, String, bool, String, String, bool, Option<String>)> =
        sqlx::query_as(
            "select id::text, slug, model, managed, department, display_name, developer, owner_user_id::text \
             from agent_defs where id = $1::uuid",
        )
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(
        |(id, slug, model, managed, department, display_name, developer, owner_user_id)| Def {
            id,
            slug,
            model,
            managed,
            department,
            display_name,
            developer,
            owner_user_id,
        },
    ))
}

/// A hostname for the card: http(s) URLs only — routing tokens
/// (talaria-pkg://, talaria-workbench://) have none.
/// The probe as one agent's tab may see it. The health pass's
/// `staleAssignments` names EVERY agent assigned to the server and the tool
/// subsets they were given — assignment data about agents this reader may
/// not read — so it stays on the registry's own (admin) surface; this tab
/// has `assignedToolsMissing` for its own agent.
fn agent_probe(probe: Option<&Value>) -> Value {
    match probe {
        Some(Value::Object(m)) => {
            let mut m = m.clone();
            m.remove("staleAssignments");
            Value::Object(m)
        }
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

fn domain_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .and_then(|u| u.host_str().map(str::to_string))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_agent_reader(&state, &headers, &id).await?;
    let def = match load_def(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[fleet/defs/mcp] def read failed", e)),
    };
    let Some(def) = def else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let can_install = talaria_users::has_perm(&state.pg, &user.id, &user.role, "agents.manage")
        .await
        .unwrap_or(false);
    let servers = match list_mcp_servers(&state.pg).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[fleet/defs/mcp] registry read failed", e)),
    };
    // This agent's assignment rows and the teams it sits on, read once.
    let assigned: Vec<(String, Option<Vec<String>>)> = match sqlx::query_as(
        "select server_id::text, tools from mcp_server_agents where agent_model = $1",
    )
    .bind(&def.model)
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return Ok(internal("[fleet/defs/mcp] assignments read failed", e)),
    };
    let assigned: HashMap<String, Option<Vec<String>>> = assigned.into_iter().collect();
    let team_grants: Vec<(String, bool)> = match sqlx::query_as(
        "select a.server_id::text, a.allowed from mcp_team_access a \
         join team_agents t on t.team_id = a.team_id where t.agent_model = $1",
    )
    .bind(&def.model)
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return Ok(internal("[fleet/defs/mcp] team grants read failed", e)),
    };
    let team_allowed: HashSet<String> = team_grants
        .iter()
        .filter(|(_, allowed)| *allowed)
        .map(|(s, _)| s.clone())
        .collect();
    // What the agent's rendered config actually lists — `carried` answers
    // from this, not from the grants alone, so the tab never shows a server
    // the agent will not list (an all-agents per-user server on an org agent,
    // a server a personal assistant's owner is denied) and never disagrees
    // with the config about `needsConnection`.
    let rendered: HashMap<String, bool> = match servers_for_agent(&state.pg, &def.model).await {
        Ok(rows) => rows
            .into_iter()
            .map(|r| (r.name, r.needs_connection))
            .collect(),
        Err(e) => return Ok(internal("[fleet/defs/mcp] rendered servers read failed", e)),
    };

    let mut out = Vec::with_capacity(servers.len());
    // `serving`: an app server whose app is off is not on any agent, so it is
    // not on this tab either. Its grants are kept for when the app serves.
    for s in servers.iter().filter(|s| s.serving()) {
        let assignment = assigned.get(&s.id);
        let granted = if s.name == WORKBENCH_SERVER {
            def.developer.then_some("developer")
        } else if s.all_agents || s.builtin {
            Some("all-agents")
        } else if assignment.is_some() {
            Some("assigned")
        } else if team_allowed.contains(&s.id) {
            Some("team")
        } else {
            None
        };
        // The builtin toolkit and the Workbench ride their own render paths
        // (servers_for_agent lists registry servers); for everything else the
        // render is the truth, and a grant it does not honour says why.
        let rides_render = !s.builtin && s.name != WORKBENCH_SERVER;
        let carried = if rides_render {
            rendered.contains_key(&s.name)
        } else {
            granted.is_some()
        };
        let via = if carried { granted } else { None };
        let withheld = match (granted, carried) {
            (Some(_), false) if def.owner_user_id.is_some() => Some("owner-denied"),
            (Some(_), false) => Some("per-user-org-agent"),
            _ => None,
        };
        // A per-user server only reaches an assistant whose owner connected.
        let needs_connection = if let Some(n) = rendered.get(&s.name).filter(|_| rides_render) {
            json!(n)
        } else if s.auth_mode == "per-user" {
            match &def.owner_user_id {
                Some(owner) => {
                    let connected = if s.oauth_enabled {
                        has_oauth_tokens(&state.pg, &s.id, owner)
                            .await
                            .unwrap_or(false)
                    } else {
                        talaria_api_facades::mcp::registry::has_user_credentials(
                            &state.pg, &s.id, owner,
                        )
                        .await
                        .unwrap_or(false)
                    };
                    json!(!connected)
                }
                // An org agent has nobody to be on a per-user server.
                None => json!(true),
            }
        } else if s.oauth_enabled && s.auth_mode == "org" {
            json!(
                !has_oauth_tokens(&state.pg, &s.id, "org")
                    .await
                    .unwrap_or(false)
            )
        } else {
            json!(false)
        };
        out.push(json!({
            "id": s.id,
            "name": s.name,
            "label": s.label,
            "description": s.description,
            // The upstream address maps the fleet's private surface; only a
            // fleet-wide reader sees even its host.
            "domain": if can_install { domain_of(&s.url).map(Value::String).unwrap_or(Value::Null) } else { Value::Null },
            "builtin": s.builtin,
            "appSlug": s.app_slug,
            "package": s.package.is_some(),
            "workbench": s.name == WORKBENCH_SERVER,
            "allAgents": s.all_agents,
            "authMode": s.auth_mode,
            "oauthEnabled": s.oauth_enabled,
            "needsConnection": needs_connection,
            "probe": agent_probe(s.last_probe.as_ref()),
            "tools": s.tools,
            "carried": carried,
            "via": via,
            // Granted, yet left out of the config: the owner of this personal
            // assistant is denied the server, or it is a per-user server that
            // reaches an org agent only by an assignment naming it.
            "withheld": withheld,
            "assigned": assignment.is_some(),
            "assignedTools": assignment.cloned().flatten(),
            // Subset entries the server's current catalog no longer offers —
            // a renamed or retired tool the gateway can never let through.
            // Live against the cached catalog (which the health pass keeps
            // current), so the tab is right the moment either side changes;
            // empty when no subset is set or the catalog was never read.
            "assignedToolsMissing": assignment
                .and_then(Option::as_deref)
                .map(|subset| missing_tools(subset, &tool_names(&s.tools)))
                .unwrap_or_default(),
        }));
    }

    // Raw entries still in the versioned config — pre-gateway leftovers.
    let legacy = match talaria_agent_mcp::list_agent_mcp(&state.pg).await {
        Ok(roster) => roster
            .into_iter()
            .find(|a| a.id == def.id)
            .map(|a| {
                a.servers
                    .into_iter()
                    .filter(|s| !s.extras.iter().any(|e| e == "built-in"))
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "url": if can_install { Value::String(s.url) } else { Value::Null },
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        Err(e) => return Ok(internal("[fleet/defs/mcp] roster read failed", e)),
    };

    Ok(Json(json!({
        "agent": { "id": def.id, "model": def.model, "slug": def.slug, "developer": def.developer, "personal": def.owner_user_id.is_some() },
        "canInstall": can_install,
        "servers": out,
        "legacy": legacy,
    }))
    .into_response())
}

/// One `assign` entry: {serverId, tools: string[] | null} — null = every tool.
struct Assign {
    server_id: String,
    tools: Option<Vec<String>>,
}

fn parse_assign(obj: &Map<String, Value>) -> Result<Vec<Assign>, String> {
    let Some(v) = obj.get("assign") else {
        return Ok(Vec::new());
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    if arr.len() > 50 {
        return Err(array_too_big_msg(50));
    }
    let mut out = Vec::new();
    for el in arr {
        let m = el
            .as_object()
            .ok_or_else(|| talaria_body::object_msg(zod_type_name(el)))?;
        let server_id = talaria_body::string_member(m, "serverId", 1, 200)?;
        let tools = match m.get("tools") {
            None | Some(Value::Null) => None,
            Some(_) => optional_string_array_member(m, "tools", 0, 120, u32::MAX as usize)?,
        };
        out.push(Assign { server_id, tools });
    }
    Ok(out)
}

/// `unassign` / `removeLegacy`: `z.array(z.string().max(200)).max(50)`.
fn parse_names(obj: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(Vec::new());
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::new();
    for el in arr {
        let s = el.as_str().ok_or_else(|| string_msg(zod_type_name(el)))?;
        if talaria_body::utf16_len(s) > 200 {
            return Err(too_big_msg(200));
        }
        out.push(s.to_string());
    }
    if arr.len() > 50 {
        return Err(array_too_big_msg(50));
    }
    Ok(out)
}

pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    if obj.get("add").is_some() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "raw MCP endpoints are not attached to agents any more: register the server in the org \
             registry (Manage → MCP, or Add from marketplace here) and assign it — agents reach \
             servers only through Talaria's gateway",
        ));
    }
    let assign = match parse_assign(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let unassign = match parse_names(obj, "unassign") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // `remove` is the old spelling of `removeLegacy`; both clean raw entries.
    let mut remove_legacy = match parse_names(obj, "removeLegacy") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match parse_names(obj, "remove") {
        Ok(v) => remove_legacy.extend(v),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
    let apply = match optional_boolean_member(obj, "apply") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
    .unwrap_or(true);

    let def = match load_def(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[fleet/defs/mcp] def read failed", e)),
    };
    let Some(def) = def else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let actor = actor_of(&user);

    // ── registry assignments ──
    let mut touched_registry = false;
    for a in &assign {
        let server = match get_mcp_server(&state.pg, &a.server_id).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &format!("MCP server \"{}\" is not registered", a.server_id),
                ));
            }
            Err(e) => return Ok(internal("[fleet/defs/mcp] server read failed", e)),
        };
        if server.name == WORKBENCH_SERVER {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "the Workbench is granted by the Developer Agent switch on this agent's settings, not by assignment",
            ));
        }
        if !server.enabled {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "\"{}\" is disabled org-wide; enable it on the MCP page first",
                    server.label
                ),
            ));
        }
        if let Err(e) = set_assignment(&state.pg, &server.id, &def.model, a.tools.as_deref()).await
        {
            return Ok(internal("[fleet/defs/mcp] assign failed", e));
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
                after: Some(json!({ "agentModel": def.model, "tools": a.tools, "from": "agent" })),
            },
        )
        .await;
        touched_registry = true;
    }
    for server_id in &unassign {
        let server = match get_mcp_server(&state.pg, server_id).await {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(e) => return Ok(internal("[fleet/defs/mcp] server read failed", e)),
        };
        if let Err(e) = remove_assignment(&state.pg, &server.id, &def.model).await {
            return Ok(internal("[fleet/defs/mcp] unassign failed", e));
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
                after: Some(json!({ "agentModel": def.model, "from": "agent" })),
            },
        )
        .await;
        touched_registry = true;
    }

    // ── legacy raw entries: a new config version without them ──
    let mut version: Option<Value> = None;
    let mut created = false;
    if !remove_legacy.is_empty() {
        let latest = match list_versions(&state.pg, &def.id).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[fleet/defs/mcp] versions read failed", e)),
        };
        let Some(latest) = latest.first() else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "no base version — import first",
            ));
        };
        let config = apply_mcp_edits(&latest.config, &def.slug, &[], &remove_legacy);
        let note = format!("mcp: -{}", remove_legacy.join(", "));
        let created_by = user
            .email
            .clone()
            .or_else(|| user.name.clone())
            .unwrap_or_else(|| "admin".into());
        match add_version_if_changed(
            &state.pg,
            &def.id,
            &talaria_agent_defs::NewVersion {
                soul: &latest.soul,
                config: &config,
                note: Some(&note),
                created_by: Some(&created_by),
            },
        )
        .await
        {
            Ok((v, c)) => {
                version = Some(json!(v));
                created = c;
            }
            Err(e) => return Ok(internal("[fleet/defs/mcp] version write failed", e)),
        }
        if created {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "agent.mcp_edit",
                    target_type: "agent",
                    target_id: Some(&def.id),
                    target_label: Some(&def.display_name),
                    before: None,
                    after: Some(json!({ "added": [], "removed": remove_legacy })),
                },
            )
            .await;
        }
    }

    // ── make it so: render, then roll the agent ──
    let mut applied = false;
    let mut warning: Option<String> = None;
    // ONE CONTAINER PER DEPARTMENT: where an older agent holds this one's
    // department too (a pair from before a department became one agent's),
    // the department's container is the other agent's and the render leaves
    // this one out. Rolling it would restart the other agent for a change
    // that is not its own, so the change is rendered and not applied.
    let runs_instead = if (touched_registry || created) && apply && def.managed {
        match talaria_agent_defs::runner_instead_of(&state.pg, &def.id, &def.department).await {
            Ok(runner) => runner,
            Err(e) => return Ok(internal("[fleet/defs/mcp] department share read failed", e)),
        }
    } else {
        None
    };
    if let Some(runner) = &runs_instead {
        let sb = secretbox_or_500(&state, "[fleet/defs/mcp] secretbox unavailable").await?;
        let _ = talaria_api_facades::fleet::render::render_fleet(&state.pg, &sb).await;
        warning = Some(talaria_agent_defs::shared_department_refusal(
            "roll",
            &def.display_name,
            &def.department,
            &runner.display_name,
        ));
    } else if (touched_registry || created) && apply && def.managed {
        let sb = secretbox_or_500(&state, "[fleet/defs/mcp] secretbox unavailable").await?;
        let _ = talaria_api_facades::fleet::render::render_fleet(&state.pg, &sb).await;
        match roll_agent(&state.pg, &sb, &def.department).await {
            Ok(None) => applied = true,
            Ok(Some(w)) => warning = Some(w),
            Err(e) => return Ok(house_error(StatusCode::BAD_REQUEST, &e)),
        }
    } else if touched_registry || created {
        // Not rolling: still re-render so the next start carries the change.
        let sb = secretbox_or_500(&state, "[fleet/defs/mcp] secretbox unavailable").await?;
        let _ = talaria_api_facades::fleet::render::render_fleet(&state.pg, &sb).await;
    }
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.insert("applied".into(), json!(applied));
    if let Some(v) = version {
        out.insert("version".into(), v);
        out.insert("created".into(), json!(created));
    }
    if let Some(w) = warning {
        out.insert("warning".into(), json!(w));
    }
    Ok(Json(Value::Object(out)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_tab_probe_never_names_other_agents() {
        let probe = json!({
            "state": "ok", "detail": "2 tool(s)", "at": "2026-10-16T00:00:00.000Z",
            "staleAssignments": [{ "agent": "finance-ops", "missing": ["x"] }],
        });
        assert_eq!(
            agent_probe(Some(&probe)),
            json!({ "state": "ok", "detail": "2 tool(s)", "at": "2026-10-16T00:00:00.000Z" })
        );
        assert_eq!(agent_probe(None), Value::Null);
        let plain = json!({ "state": "unreachable", "detail": "d", "at": "t" });
        assert_eq!(agent_probe(Some(&plain)), plain);
    }
}

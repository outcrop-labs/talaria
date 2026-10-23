// /api/fleet/defs/{id}. PATCH → editable agent identity metadata (role,
// display name, send alias) plus the Developer Agent switch, the delegation
// cap, the toolsets the agent does not get, and template binds. Not
// versioned: this is identity, not config. Admin only.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;
use talaria_agent_defs::{
    AgentMetaPatch, apply_developer_toolsets, set_disabled_toolsets, set_max_children,
    set_pinned_sessions, update_agent_meta, validate_toolsets,
};
use talaria_api_facades::mcp::apply::roll_agent_for_model;
use talaria_api_facades::workbench::set_developer;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    NumKind, optional_boolean_member, optional_number_member, optional_string_array_member, parse,
    present_nullable_max_string_member, present_nullable_uuid_member, string_msg, too_big_msg,
    too_small_msg, utf16_len, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_agent_manager};
use talaria_state::AppState;
use talaria_templates::set_agent_templates;

/// `displayName` — min(1).max(80), optional only, so null is a type error
/// like any other non-string.
fn parse_display_name(obj: &Map<String, Value>) -> Result<Option<String>, String> {
    match obj.get("displayName") {
        None => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            if s.chars().count() < 1 {
                return Err(too_small_msg(1));
            }
            if utf16_len(s) > 80 {
                return Err(too_big_msg(80));
            }
            Ok(Some(s.to_string()))
        }
    }
}

/// `emailAlias`: trimmed, ≤320, then a bare-address check — Gmail rejects a
/// From it doesn't own (which the send surfaces anyway), but a typo'd shape
/// should never even save. Nullish: absent leaves the column, null derives
/// the org account's plus-address for the slug.
fn parse_email_alias(obj: &Map<String, Value>) -> Result<Option<Option<String>>, String> {
    static EMAIL_ALIAS: OnceLock<regex::Regex> = OnceLock::new();
    match obj.get("emailAlias") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            let trimmed = s.trim();
            if utf16_len(trimmed) > 320 {
                return Err(too_big_msg(320));
            }
            let ok = EMAIL_ALIAS
                .get_or_init(|| regex::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap())
                .is_match(trimmed);
            if !ok {
                return Err("not an email address".into());
            }
            Ok(Some(Some(trimmed.to_string())))
        }
    }
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let role = match present_nullable_max_string_member(obj, "role", 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let display_name = match parse_display_name(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let email_alias = match parse_email_alias(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Template overrides: uuid binds, null clears, omitted leaves unchanged.
    let ticket_template_id = match present_nullable_uuid_member(obj, "ticketTemplateId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let plan_template_id = match present_nullable_uuid_member(obj, "planTemplateId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The Developer Agent switch: on sets the agent up end to end (sandbox,
    // Oh My Pi, Workbench tools); the roll below applies it.
    let developer = match optional_boolean_member(obj, "developer") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // How many subagents this agent may run at once; 0 means it delegates to
    // nobody. The ceiling is the box's, not the agent's — the roll applies it.
    let max_children = match optional_number_member(obj, "maxChildren", NumKind::Int, 0.0, 16.0) {
        // Narrowed here, once: the member reader speaks f64 for every numeric
        // shape, and the column, the audit and the renderer all want the int.
        Ok(v) => v.map(|n| n as i32),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Pinned sessions: one Hermes session per conversation, Hermes holding the
    // history, so its compaction and session search see the whole thread. Talaria-side only — no roll.
    let pinned_sessions = match optional_boolean_member(obj, "pinnedSessions") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The Hermes toolsets this agent does NOT get — the whole list, or absent. Only governed names pass:
    // a stranger reaches the rendered config, is ignored in silence, and the toolset stays open. It rolls.
    let disabled_toolsets = match optional_string_array_member(obj, "disabledToolsets", 1, 40, 40)
        .and_then(|names| names.map(|n| validate_toolsets(&n)).transpose())
    {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // The def read: the columns this route touches. The body parses BEFORE
    // the lookup, so a bad body on an unknown id is the 400, not the 404.
    let def: Option<(String, String, String, i32, Value)> = match sqlx::query_as(
        "select id::text, model, display_name, max_children, disabled_toolsets from agent_defs \
         where id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => return Ok(internal("[fleet/defs] def read failed", e)),
    };
    let Some((def_id, def_model, def_display_name, def_max_children, def_disabled)) = def else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };

    if let Err(e) = update_agent_meta(
        &state.pg,
        &def_id,
        &AgentMetaPatch {
            role: role.as_ref().map(|o| o.as_deref()),
            display_name: display_name.as_deref(),
            email_alias: email_alias.as_ref().map(|o| o.as_deref()),
        },
    )
    .await
    {
        return Ok(internal("[fleet/defs] meta update failed", e));
    }
    let developer_changed = match developer {
        Some(on) => match set_developer(&state.pg, &def_id, on).await {
            Ok(changed) => changed,
            Err(e) => return Ok(internal("[fleet/defs] developer set failed", e)),
        },
        None => false,
    };
    // The switch's toolset half: on opens terminal/file/code_execution (the
    // closed default keeps them off every other agent), off closes them
    // again. Applied on a CHANGE only, so re-saving `developer: true` on an
    // agent whose manager since closed `file` does not reopen it. It is
    // applied BEFORE an explicit `disabledToolsets` in the same PATCH, so a
    // caller who sends both gets the list they sent.
    if developer_changed
        && let Some(on) = developer
        && let Err(e) = apply_developer_toolsets(&state.pg, &def_id, on).await
    {
        return Ok(internal("[fleet/defs] developer toolsets failed", e));
    }
    let max_children_changed = match max_children {
        Some(n) => match set_max_children(&state.pg, &def_id, n).await {
            Ok(changed) => changed,
            Err(e) => return Ok(internal("[fleet/defs] max children set failed", e)),
        },
        None => false,
    };
    let pinned_changed = match pinned_sessions {
        Some(on) => match set_pinned_sessions(&state.pg, &def_id, on).await {
            Ok(changed) => changed,
            Err(e) => return Ok(internal("[fleet/defs] pinned sessions set failed", e)),
        },
        None => false,
    };
    let toolsets_changed = match &disabled_toolsets {
        Some(names) => match set_disabled_toolsets(&state.pg, &def_id, names).await {
            Ok(changed) => changed,
            Err(e) => return Ok(internal("[fleet/defs] disabled toolsets set failed", e)),
        },
        None => false,
    };
    if ticket_template_id.is_some() || plan_template_id.is_some() {
        // Template binds key on the agent's MODEL, not its id — the same
        // identity the chain resolves by.
        if let Err(e) = set_agent_templates(
            &state.pg,
            &def_model,
            ticket_template_id.as_ref().map(|o| o.as_deref()),
            plan_template_id.as_ref().map(|o| o.as_deref()),
        )
        .await
        {
            return Ok(internal("[fleet/defs] template bind failed", e));
        }
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.meta",
            target_type: "agent",
            target_id: Some(&def_id),
            target_label: Some(&def_display_name),
            before: None,
            after: None,
        },
    )
    .await;
    if pinned_changed && let Some(on) = pinned_sessions {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.pinned_sessions",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&def_display_name),
                before: Some(json!({ "pinnedSessions": !on })),
                after: Some(json!({ "pinnedSessions": on })),
            },
        )
        .await;
    }
    if toolsets_changed && let Some(names) = &disabled_toolsets {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.toolsets",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&def_display_name),
                before: Some(json!({ "disabledToolsets": def_disabled })),
                after: Some(json!({ "disabledToolsets": names })),
            },
        )
        .await;
    }
    // The pinned flip changes how TALARIA drives the agent — which Hermes
    // endpoint the next turn goes to — and nothing in the container, so it
    // takes effect on the next turn with no roll. The Developer switch is
    // the one that rolls: it changes the container's env and image, which
    // only a recreate delivers. Fire-and-forget, never the caller's problem.
    //
    // A toolset change rolls in the same stroke. Hermes itself reads
    // `agent.disabled_toolsets` per request from the config.yaml the render
    // just rewrote in place, so the roll is not what closes a toolset — but
    // opening `browser` has to pay for the setup the post-health step runs
    // on a fresh container. Both switches in one PATCH are still ONE roll.
    //
    // THE ROLL NEVER FAILS THE PATCH. The switch is already stored by the
    // time the roll is queued, and a box the api cannot open is the roll's
    // problem to log, not the caller's to see as a 500 for a change that
    // landed — the next render picks the stored value up regardless.
    if developer_changed || toolsets_changed || max_children_changed {
        match state.secretbox().await {
            Ok(sb) => {
                let (pg, model) = (state.pg.clone(), def_model.clone());
                tokio::spawn(async move {
                    roll_agent_for_model(&pg, &sb, &model).await;
                });
            }
            Err(e) => tracing::warn!(
                "[fleet/defs] {def_id}: switch stored but the roll could not start — secretbox unavailable: {e}"
            ),
        }
    }
    if developer_changed && let Some(on) = developer {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.developer",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&def_display_name),
                before: Some(json!({ "developer": !on })),
                after: Some(json!({ "developer": on })),
            },
        )
        .await;
    }
    if max_children_changed && let Some(n) = max_children {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.max_children",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&def_display_name),
                before: Some(json!({ "maxChildren": def_max_children })),
                after: Some(json!({ "maxChildren": n })),
            },
        )
        .await;
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

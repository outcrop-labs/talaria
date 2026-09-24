// /api/fleet/defs/{id}. PATCH → editable agent identity metadata (role,
// display name, send alias) plus the Developer Agent switch and template
// binds. Not versioned: this is identity, not config. Admin only.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;
use talaria_agent_defs::{AgentMetaPatch, update_agent_meta};
use talaria_api_facades::mcp::apply::roll_agent_for_model;
use talaria_api_facades::workbench::set_developer;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    optional_boolean_member, parse, present_nullable_max_string_member,
    present_nullable_uuid_member, string_msg, too_big_msg, too_small_msg, utf16_len, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_perm, secretbox_or_500};
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
    let user = require_perm(&state, &headers, "agents.manage").await?;
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

    // The def read: the columns this route touches. The body parses BEFORE
    // the lookup, so a bad body on an unknown id is the 400, not the 404.
    let def: Option<(String, String, String)> = match sqlx::query_as(
        "select id::text, model, display_name from agent_defs where id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => return Ok(internal("[fleet/defs] def read failed", e)),
    };
    let Some((def_id, def_model, def_display_name)) = def else {
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
        // A running Hermes wires its MCP servers and container env at start,
        // so the switch lands by rolling the agent (blue/green; the roll
        // re-renders the incoming slot). Fire-and-forget, never the caller's
        // problem.
        let sb = secretbox_or_500(&state, "[fleet/defs] secretbox unavailable").await?;
        let (pg, model) = (state.pg.clone(), def_model.clone());
        tokio::spawn(async move {
            roll_agent_for_model(&pg, &sb, &model).await;
        });
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

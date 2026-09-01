// /api/fleet/defs/{id} — port of ui/src/routes/api/fleet.defs.$id.ts.
// PATCH → editable agent identity metadata (role, display name, send alias)
// plus the workbench and template binds. Not versioned — this is identity,
// not config. Admin only.

use crate::agent_defs::{AgentMetaPatch, update_agent_meta};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, object_msg, optional_enum_member, parse, present_nullable_max_string_member,
    present_nullable_uuid_member, string_msg, too_big_msg, too_small_msg, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_perm};
use crate::state::AppState;
use crate::templates::set_agent_templates;
use crate::workbench::{set_agent_workbench, set_agent_workbench_tuning};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;

/// `displayName: z.string().min(1).max(80).optional()` — optional only, so
/// null is a type error like any other non-string.
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

/// `emailAlias`: trimmed, ≤320, then a bare-address refine — Gmail rejects a
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

/// `workbenchModels`: three nullable-optional model picks, one per weight
/// class. zod strips unknown keys, so the stored map carries only these
/// three, present ones only.
fn parse_workbench_models(obj: &Map<String, Value>) -> Result<Option<Map<String, Value>>, String> {
    match obj.get("workbenchModels") {
        None => Ok(None),
        Some(v) => {
            let m = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
            let mut out = Map::new();
            for key in ["light", "standard", "heavy"] {
                match m.get(key) {
                    None => {}
                    Some(Value::Null) => {
                        out.insert(key.into(), Value::Null);
                    }
                    Some(sv) => {
                        let s = sv.as_str().ok_or_else(|| string_msg(zod_type_name(sv)))?;
                        if utf16_len(s) > 200 {
                            return Err(too_big_msg(200));
                        }
                        out.insert(key.into(), json!(s));
                    }
                }
            }
            Ok(Some(out))
        }
    }
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let role = match present_nullable_max_string_member(obj, "role", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let display_name = match parse_display_name(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let email_alias = match parse_email_alias(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Template overrides: uuid binds, null clears, omitted leaves unchanged.
    let ticket_template_id = match present_nullable_uuid_member(obj, "ticketTemplateId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let plan_template_id = match present_nullable_uuid_member(obj, "planTemplateId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let workbench = match optional_enum_member(obj, "workbench", &["off", "auto", "on"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let workbench_profile = match present_nullable_max_string_member(obj, "workbenchProfile", 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let workbench_harness = match present_nullable_max_string_member(obj, "workbenchHarness", 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let workbench_models = match parse_workbench_models(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // getAgentDef — the columns this route touches (the workbench fallback
    // below needs the stored value). TS parses the body BEFORE the lookup, so
    // a bad body on an unknown id is the 400, not the 404.
    let def: Option<(String, String, String, Option<String>)> = match sqlx::query_as(
        "select id::text, model, display_name, workbench from agent_defs where id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!("[fleet/defs] def read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((def_id, def_model, def_display_name, def_workbench)) = def else {
        return house_error(StatusCode::NOT_FOUND, "not found");
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
        tracing::error!("[fleet/defs] meta update failed: {e}");
        return thrown_internal_error();
    }
    if workbench.is_some() || workbench_profile.is_some() {
        // `body.workbench ?? def.workbench ?? 'auto'` — a profile-only patch
        // re-states the stored mode rather than defaulting it.
        let wb = workbench.or(def_workbench).unwrap_or_else(|| "auto".into());
        if let Err(e) = set_agent_workbench(
            &state.pg,
            &def_id,
            &wb,
            workbench_profile.as_ref().map(|o| o.as_deref()),
        )
        .await
        {
            tracing::error!("[fleet/defs] workbench set failed: {e}");
            return thrown_internal_error();
        }
    }
    if (workbench_harness.is_some() || workbench_models.is_some())
        && let Err(e) = set_agent_workbench_tuning(
            &state.pg,
            &def_id,
            workbench_harness.as_ref().map(|o| o.as_deref()),
            workbench_models.as_ref(),
        )
        .await
    {
        tracing::error!("[fleet/defs] workbench tuning failed: {e}");
        return thrown_internal_error();
    }
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
            tracing::error!("[fleet/defs] template bind failed: {e}");
            return thrown_internal_error();
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
    Json(json!({ "ok": true })).into_response()
}

// /api/admin/model-roles. Model Roles — which model handles each activity
// class. GET → the catalog of roles + current assignments + assignable
// models + fitness issues. PUT { role, model|null } → assign (null = back
// to auto). Admins only.
//
// `issues` is the audit-1.6 signal and it is ADVISORY on both verbs: a PUT that
// creates one still succeeds, and answers with the issue it just created so the
// panel can say so on the same round trip. Refusing the assignment would put
// this route in the business of overruling an admin on the strength of a probe,
// which is a call the admin is better placed to make.
//
// No transaction: the model write and the effort write are two settings rows
// touched in sequence, and a refused effort (400) does NOT roll the model
// write back — the assignment stands, the panel re-reads it, and the admin
// sees both halves of what they did.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use talaria_api_facades::model::access::gateway_models;
use talaria_api_facades::model::efforts::efforts_for_model;
use talaria_api_facades::model::roles::{MODEL_ROLES, get_model_roles, set_model_role};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    enum_member, parse, present_nullable_max_string_member, present_nullable_string_member,
};
use talaria_capability_reach::role_assignment_issues_reached;
use talaria_effort_prefs::{get_effort_prefs, role_slot, set_effort_pref};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

const ROLES: [&str; 11] = [
    "research-recon",
    "research-brief",
    "research-expedition",
    "utility",
    "code-light",
    "code-standard",
    "code-heavy",
    "vision",
    "image-generation",
    "embedding",
    "reranker",
];

/// The role catalog as the panel renders it — key order included.
fn role_specs() -> Vec<Value> {
    MODEL_ROLES
        .iter()
        .map(|r| {
            json!({
                "role": r.role,
                "label": r.label,
                "hint": r.hint,
                "wired": r.wired,
                "requires": r.requires,
            })
        })
        .collect()
}

async fn efforts_panel(pg: &PgPool) -> serde_json::Map<String, Value> {
    let prefs = get_effort_prefs(pg).await;
    let mut out = Map::new();
    for r in MODEL_ROLES.iter() {
        let v = prefs
            .get(&role_slot(r.role))
            .cloned()
            .unwrap_or(Value::Null);
        out.insert(r.role.to_string(), v);
    }
    out
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let models = match gateway_models(&state.pg).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[model-roles] gateway catalog read failed", e)),
    };
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    let issues: Vec<Value> = role_assignment_issues_reached(&state.pg)
        .await
        .iter()
        .map(|i| i.to_json())
        .collect();
    Ok(Json(json!({
        "roles": role_specs(),
        "assignments": get_model_roles(&state.pg).await,
        "models": ids,
        "issues": issues,
        // The per-role effort preference (null = the model's own default).
        "efforts": Value::Object(efforts_panel(&state.pg).await),
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Schema order: role, then model, then effort — an invalid role outranks
    // an invalid model, which outranks an invalid effort.
    let role = match enum_member(obj, "role", &ROLES) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let model = match present_nullable_max_string_member(obj, "model", 200) {
        Ok(m) => m,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Absent = leave the preference alone (a model-only save); null = clear.
    let effort = match present_nullable_string_member(obj, "effort", 24) {
        Ok(e) => e,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(Some(m)) = &model
        && !m.is_empty()
    {
        let on_gateway = match gateway_models(&state.pg).await {
            Ok(all) => all.iter().any(|g| &g.id == m),
            Err(e) => return Ok(internal("[model-roles] gateway catalog read failed", e)),
        };
        if !on_gateway {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "that model is not on the gateway",
            ));
        }
    }
    if let Some(model_value) = &model {
        if let Err(e) = set_model_role(&state.pg, &role, model_value.as_deref()).await {
            return Ok(internal("[model-roles] assignment write failed", e));
        }
        // after: { model: body.model } — the RAW parsed value, so present-null
        // audits {model: null} and an explicit "" audits {model: ""}.
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "model_role.assign",
                target_type: "model-role",
                target_id: Some(&role),
                target_label: None,
                before: None,
                after: Some(json!({ "model": model_value })),
            },
        )
        .await;
    }
    if let Some(effort_value) = &effort {
        // Validated against the levels the model PUBLISHES: a preference for a
        // level this model has never supported is a typo, and storing it would
        // render a picker that silently no-ops at run time. Auto (no model
        // assigned) has no ladder to validate against and no turn to ride, so
        // it is refused rather than parked. The target is the model THIS body
        // names — including "" — else the standing assignment re-read AFTER
        // the write above (so an assign-and-set-effort in one request
        // validates against the fresh assignment).
        let target: Option<String> = match &model {
            Some(m) if m.is_some() => m.clone(),
            _ => get_model_roles(&state.pg)
                .await
                .get(role.as_str())
                .and_then(|v| v.as_str())
                .map(String::from),
        };
        let Some(chosen) = effort_value.as_deref().filter(|e| !e.is_empty()) else {
            // null (or "") = clear the preference; nothing to validate.
            if let Err(e) = set_effort_pref(&state.pg, &role_slot(&role), None).await {
                return Ok(internal("[model-roles] effort pref write failed", e));
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor_of(&user),
                    action: "model_role.effort",
                    target_type: "model-role",
                    target_id: Some(&role),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "effort": effort_value })),
                },
            )
            .await;
            return Ok(finish(&state).await);
        };
        let Some(target) = target.filter(|t| !t.is_empty()) else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "assign a model before setting its effort",
            ));
        };
        if !efforts_for_model(&state.pg, &target)
            .await
            .contains(&chosen.to_string())
        {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("that model does not publish the \"{chosen}\" effort level"),
            ));
        }
        if let Err(e) = set_effort_pref(&state.pg, &role_slot(&role), Some(chosen)).await {
            return Ok(internal("[model-roles] effort pref write failed", e));
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "model_role.effort",
                target_type: "model-role",
                target_id: Some(&role),
                target_label: None,
                before: None,
                after: Some(json!({ "effort": effort_value })),
            },
        )
        .await;
    }
    Ok(finish(&state).await)
}

async fn finish(state: &AppState) -> Response {
    let issues: Vec<Value> = role_assignment_issues_reached(&state.pg)
        .await
        .iter()
        .map(|i| i.to_json())
        .collect();
    Json(json!({
        "assignments": get_model_roles(&state.pg).await,
        "issues": issues,
        "efforts": Value::Object(efforts_panel(&state.pg).await),
    }))
    .into_response()
}

// /api/agent-role-templates — port of ui/src/routes/api/agent-role-templates.ts.
// The business roles a new agent can start from.
//   GET    → built-ins + the org's own (anyone who may create an agent).
//   PUT    → create or update an ORG template (admin; it seeds every future agent).
//   DELETE → remove an org template; a shadowed built-in reappears.

use crate::agent_role_templates::{
    RoleTemplateInput, delete_role_template, list_role_templates, upsert_role_template,
};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, kebab_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_admin, require_perm};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(serde::Deserialize)]
pub struct SlugQuery {
    slug: Option<String>,
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    match list_role_templates(&state.pg).await {
        Ok(templates) => Json(json!({ "templates": templates })).into_response(),
        Err(e) => {
            tracing::error!("[agent-role-templates] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let slug = match kebab_member(obj, "slug") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let role = match string_member(obj, "role", 1, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let department = match kebab_member(obj, "department") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.string().max(300).default('') — no min: '' is a legal description,
    // and an absent one IS ''. `.default` only fills UNDEFINED; a present
    // null still fails the type check, exactly as zod does it.
    let description = match obj.get("description") {
        None => String::new(),
        Some(_) => match string_member(obj, "description", 0, 300) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        },
    };
    let soul = match string_member(obj, "soul", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let input = RoleTemplateInput {
        slug: slug.clone(),
        name: name.clone(),
        role,
        department,
        description,
        soul,
    };
    let template = match upsert_role_template(&state.pg, &input, &actor_of(&user)).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[agent-role-templates] upsert failed: {e}");
            return thrown_internal_error();
        }
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.role_template_save",
            target_type: "agent-role-template",
            target_id: Some(&slug),
            target_label: Some(&name),
            before: None,
            after: None,
        },
    )
    .await;
    Json(json!({ "template": template })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<SlugQuery>,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let Some(slug) = q.slug.filter(|s| !s.is_empty()) else {
        return house_error(StatusCode::BAD_REQUEST, "slug required");
    };
    match delete_role_template(&state.pg, &slug).await {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[agent-role-templates] delete failed: {e}");
            return thrown_internal_error();
        }
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.role_template_delete",
            target_type: "agent-role-template",
            target_id: Some(&slug),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}

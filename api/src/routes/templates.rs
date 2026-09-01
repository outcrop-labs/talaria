// /api/templates — port of ui/src/routes/api/templates.ts.
// The org's template library (ticket + plan formats). GET → all (any member —
// the library grounds pickers everywhere). POST → create (any member, like
// boards/channels; the skeletons are org-shared working material, not
// policy).

use crate::body::{as_object, optional_max_string_member, parse, trimmed_string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{require_perm, require_user};
use crate::state::AppState;
use crate::templates::{NewTemplate, create_template, list_templates};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    match list_templates(&state.pg, None).await {
        Ok(templates) => Json(json!({ "templates": templates })).into_response(),
        Err(e) => {
            tracing::error!("[templates] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "templates.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match trimmed_string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let kind = match crate::body::enum_member(obj, "kind", &["ticket", "plan"]) {
        Ok(k) => k,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body_text = match optional_max_string_member(obj, "body", 50_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let guidance = match optional_max_string_member(obj, "guidance", 10_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let created_by = user.email.as_deref().or(user.name.as_deref()).unwrap_or("user");
    match create_template(
        &state.pg,
        NewTemplate {
            name: &name,
            kind: &kind,
            body: body_text.as_deref(),
            guidance: guidance.as_deref(),
            created_by,
        },
    )
    .await
    {
        Ok(template) => Json(json!({ "template": template })).into_response(),
        Err(e) => {
            tracing::error!("[templates] create failed: {e}");
            thrown_internal_error()
        }
    }
}

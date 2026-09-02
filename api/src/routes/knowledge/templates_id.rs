// /api/templates/{id}. One template: PUT → edit (kind is immutable — retire
// and recreate instead), DELETE → remove (bindings cascade/null out;
// consumers fall through the chain).

use crate::body::{as_object, optional_max_string_member, parse, trimmed_string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_perm;
use crate::state::AppState;
use crate::templates::{TemplatePatch, delete_template, get_template, update_template};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let name = match optional_trimmed(obj, "name", 120) {
        Ok(v) => v,
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
    let author = user.email.as_deref().or(user.name.as_deref());
    match update_template(
        &state.pg,
        &id,
        TemplatePatch {
            name: name.as_deref(),
            body: body_text.as_deref(),
            guidance: guidance.as_deref(),
            author,
        },
    )
    .await
    {
        Ok(Some(template)) => Json(json!({ "template": template })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[templates] update failed: {e}");
            thrown_internal_error()
        }
    }
}

/// Optional trimmed string — the trim runs before the bounds, and only when
/// the key is present.
fn optional_trimmed(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(_) => trimmed_string_member(obj, key, 1, max).map(Some),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "templates.manage").await {
        return gate;
    }
    match get_template(&state.pg, &id).await {
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Ok(Some(_)) => match delete_template(&state.pg, &id).await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => {
                tracing::error!("[templates] delete failed: {e}");
                thrown_internal_error()
            }
        },
        Err(e) => {
            tracing::error!("[templates] read before delete failed: {e}");
            thrown_internal_error()
        }
    }
}

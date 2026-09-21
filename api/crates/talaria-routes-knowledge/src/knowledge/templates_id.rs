// /api/templates/{id}. One template: PUT → edit (kind is immutable — retire
// and recreate instead), DELETE → remove (bindings cascade/null out;
// consumers fall through the chain).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{optional_max_string_member, parse, trimmed_string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_perm;
use talaria_state::AppState;
use talaria_templates::{TemplatePatch, delete_template, get_template, update_template};

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "templates.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match optional_trimmed(obj, "name", 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let body_text = match optional_max_string_member(obj, "body", 50_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let guidance = match optional_max_string_member(obj, "guidance", 10_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let author = user.email.as_deref().or(user.name.as_deref());
    Ok(
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
            Err(e) => internal("[templates] update failed", e),
        },
    )
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
) -> Result<Response, Response> {
    require_perm(&state, &headers, "templates.manage").await?;
    Ok(match get_template(&state.pg, &id).await {
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Ok(Some(_)) => match delete_template(&state.pg, &id).await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => internal("[templates] delete failed", e),
        },
        Err(e) => internal("[templates] read before delete failed", e),
    })
}

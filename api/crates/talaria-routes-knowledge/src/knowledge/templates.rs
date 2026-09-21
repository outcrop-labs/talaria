// /api/templates. The org's template library (ticket + plan formats). GET →
// all (any member — the library grounds pickers everywhere). POST → create
// (needs templates.manage — the skeletons are org-wide starting points).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{optional_max_string_member, parse, trimmed_string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{require_perm, require_user};
use talaria_state::AppState;
use talaria_templates::{NewTemplate, create_template, list_templates};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_user(&state, &headers).await?;
    Ok(match list_templates(&state.pg, None).await {
        Ok(templates) => Json(json!({ "templates": templates })).into_response(),
        Err(e) => internal("[templates] list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "templates.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match trimmed_string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let kind = match talaria_body::enum_member(obj, "kind", &["ticket", "plan"]) {
        Ok(k) => k,
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
    let created_by = user
        .email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("user");
    Ok(
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
            Err(e) => internal("[templates] create failed", e),
        },
    )
}

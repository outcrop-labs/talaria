// /api/boards/{id}/templates. The ticket templates a board uses. GET → the
// bindings (any member); PUT
// {templateIds, defaultId} → replace the set (owner/editor). defaultId must
// be one of templateIds (null = no default); an empty templateIds clears the
// board's bindings. No publish and no audit — a binding change is the board's
// own configuration, not a board event and not an admin action.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_boards::{board_role, can_edit};
use talaria_body::{as_object, nullable_uuid_member, parse, uuid_array_member};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_templates::{board_templates, set_board_templates};

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET templates", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => return internal("[boards] role read on templates failed", e),
    }
    match board_templates(&state.pg, &id).await {
        Ok(bindings) => Json(json!({ "bindings": bindings })).into_response(),
        Err(e) => internal("[boards] template bindings read failed", e),
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "PUT templates", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => return internal("[boards] role read on template put failed", e),
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let template_ids = match uuid_array_member(obj, "templateIds", 50) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let default_id = match nullable_uuid_member(obj, "defaultId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // a null defaultId skips the check; a real one must name a member of the
    // set it travels with.
    if let Some(def) = &default_id
        && !template_ids.contains(def)
    {
        return house_error(
            StatusCode::BAD_REQUEST,
            "defaultId must be one of templateIds",
        );
    }
    if let Err(e) = set_board_templates(&state.pg, &id, &template_ids, default_id.as_deref()).await
    {
        return internal("[boards] template bindings write failed", e);
    }
    // The fresh set rides back — the write and the answer are one read apart,
    // so a stale client cannot keep rendering a binding the board dropped.
    match board_templates(&state.pg, &id).await {
        Ok(bindings) => Json(json!({ "bindings": bindings })).into_response(),
        Err(e) => internal("[boards] template bindings reread failed", e),
    }
}

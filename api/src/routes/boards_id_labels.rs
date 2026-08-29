// /api/boards/{id}/labels — port of ui/src/routes/api/boards.$id.labels.ts.
// Board labels: GET → the registry (any member); POST create, PUT rename/
// recolor (a rename cascades into tickets), DELETE (strips off tickets) —
// owner/editor. The throws the TS server functions make ('label name
// required', 'no such label', 'unknown color') are caught by the route and
// answered as 400s, which is why they ride as inner Results in labels.rs.

use crate::boards::{board_role, can_edit};
use crate::body::{
    as_object, optional_max_string_member, optional_string_member, parse, string_member,
    uuid_member,
};
use crate::error::house_error;
use crate::labels::{create_label, delete_label, list_labels, update_label};
use crate::realtime::RealtimeDeps;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

async fn role_gate(state: &AppState, user_id: &str, id: &str, action: &str) -> Option<Response> {
    match board_role(&state.pg, user_id, id).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[boards] role read on {action} failed: {e}");
            Some(house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

async fn edit_gate(state: &AppState, user_id: &str, id: &str, action: &str) -> Option<Response> {
    match board_role(&state.pg, user_id, id).await {
        Ok(role) if can_edit(role.as_deref()) => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[boards] role read on {action} failed: {e}");
            Some(house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET labels", &id) {
        return gate;
    }
    if let Some(gate) = role_gate(&state, &user.id, &id, "GET labels").await {
        return gate;
    }
    match list_labels(&state.pg, &id).await {
        Ok(labels) => Json(json!({ "labels": labels })).into_response(),
        Err(e) => {
            tracing::error!("[boards] label list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "POST labels", &id) {
        return gate;
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "POST labels").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // color is a free string on the wire (≤20); the palette decides what it
    // means — anything off it coerces to slate inside create_label.
    let color = match optional_max_string_member(obj, "color", 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match create_label(&state.pg, &id, &name, color.as_deref()).await {
        Ok(Ok(label)) => Json(json!({ "label": label })).into_response(),
        Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
        Err(e) => {
            tracing::error!("[boards] label create failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
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
    if let Some(gate) = crate::params::uuid_gate("boards", "PUT labels", &id) {
        return gate;
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "PUT labels").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let label_id = match uuid_member(obj, "labelId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_string_member(obj, "name", 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let color = match optional_max_string_member(obj, "color", 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    match update_label(
        &state.pg,
        &realtime,
        &id,
        &label_id,
        name.as_deref(),
        color.as_deref(),
    )
    .await
    {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
        Err(e) => {
            tracing::error!("[boards] label update failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "DELETE labels", &id) {
        return gate;
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "DELETE labels").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let label_id = match uuid_member(obj, "labelId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    if let Err(e) = delete_label(&state.pg, &realtime, &id, &label_id).await {
        tracing::error!("[boards] label delete failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    Json(json!({ "ok": true })).into_response()
}

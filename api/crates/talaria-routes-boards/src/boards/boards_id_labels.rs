// /api/boards/{id}/labels. Board labels: GET → the registry (any member);
// POST create, PUT rename/recolor (a rename cascades into tickets), DELETE
// (strips off tickets) — owner/editor. The label helpers' refusal sentences
// ('label name required', 'no such label', 'unknown color') answer as 400s —
// they ride as inner Results out of labels.rs for exactly that.

use super::edit_gate;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_boards::board_role;
use talaria_body::{
    optional_max_string_member, optional_string_member, parse, string_member, uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_labels::{create_label, delete_label, list_labels, update_label};
use talaria_realtime_watch::RealtimeDeps;
use talaria_session::require_user;
use talaria_state::AppState;

async fn role_gate(state: &AppState, user_id: &str, id: &str, action: &str) -> Option<Response> {
    match board_role(&state.pg, user_id, id).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => Some(internal(
            &format!("[boards] role read on {action} failed"),
            e,
        )),
    }
}
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET labels", &id) {
        return Ok(gate);
    }
    if let Some(gate) = role_gate(&state, &user.id, &id, "GET labels").await {
        return Ok(gate);
    }
    Ok(match list_labels(&state.pg, &id).await {
        Ok(labels) => Json(json!({ "labels": labels })).into_response(),
        Err(e) => internal("[boards] label list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST labels", &id) {
        return Ok(gate);
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "POST labels").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match string_member(obj, "name", 1, 40) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // color is a free string on the wire (≤20); the palette decides what it
    // means — anything off it coerces to slate inside create_label.
    let color = match optional_max_string_member(obj, "color", 20) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match create_label(&state.pg, &id, &name, color.as_deref()).await {
            Ok(Ok(label)) => Json(json!({ "label": label })).into_response(),
            Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
            Err(e) => internal("[boards] label create failed", e),
        },
    )
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "PUT labels", &id) {
        return Ok(gate);
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "PUT labels").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let label_id = match uuid_member(obj, "labelId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let name = match optional_string_member(obj, "name", 40) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let color = match optional_max_string_member(obj, "color", 20) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    Ok(
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
            Err(e) => internal("[boards] label update failed", e),
        },
    )
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "DELETE labels", &id) {
        return Ok(gate);
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "DELETE labels").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let label_id = match uuid_member(obj, "labelId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    if let Err(e) = delete_label(&state.pg, &realtime, &id, &label_id).await {
        return Ok(internal("[boards] label delete failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

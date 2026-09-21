// /api/boards/{id}/workchains. The board's workchains — ordered pipelines
// of its tickets. GET → the chains with their steps joined to task
// summaries and the derived done/head/waiting state (any member, exactly
// the readers board configuration gets); POST { name } → create, positioned
// after the last chain — owner/editor.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_boards::{board_role, can_edit};
use talaria_body::{as_object, parse, string_member};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_workchains::{Workchain, list_workchains};

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET workchains", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => return internal("[boards] role read on GET workchains failed", e),
    }
    match list_workchains(&state.pg, &id).await {
        Ok(workchains) => Json(json!({ "workchains": workchains })).into_response(),
        Err(e) => internal("[boards] workchain list failed", e),
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
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST workchains", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => return internal("[boards] role read on POST workchains failed", e),
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // createdBy is the human-readable attribution: email, else name, else
    // 'user' — the same actor ladder the audit log climbs.
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    type ChainRow = (String, String, String, Option<String>, bool, i32, i64, i64);
    let row: ChainRow = match sqlx::query_as(
        "insert into task_workchains (board_id, name, created_by, position) \
         values ($1::uuid, $2, $3, \
                 coalesce((select max(position) + 1 from task_workchains where board_id = $1::uuid), 0)) \
         returning id::text, board_id::text, name, created_by, paused, position, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint, \
                   (trunc(extract(epoch from updated_at) * 1000))::bigint",
    )
    .bind(&id)
    .bind(&name)
    .bind(&created_by)
    .fetch_one(&state.pg)
    .await
    {
        Ok(v) => v,
        Err(e) => return internal("[boards] workchain create failed", e)
    };
    let (wid, board_id, name, created_by, paused, position, created_ms, updated_ms) = row;
    Json(json!({
        "workchain": Workchain {
            id: wid,
            board_id,
            name,
            created_by,
            paused,
            position,
            created_at: talaria_agent_auth::epoch_ms_to_iso(created_ms),
            updated_at: talaria_agent_auth::epoch_ms_to_iso(updated_ms),
            steps: vec![],
        }
    }))
    .into_response()
}

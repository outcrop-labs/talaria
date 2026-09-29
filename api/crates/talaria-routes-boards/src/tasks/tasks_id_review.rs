// /api/tasks/{id}/review. The human quality gate. Approve MARKS the ticket
// signed off and leaves it where it is; reject sends it back to the board's
// first working column. Board owner/editor only.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_boards::{board_role, can_edit};
use talaria_body::{optional_max_string_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_statuses::status_meta;
use talaria_tasks::{
    TaskActor, TaskDeps, TaskPatch, add_review, get_task, set_task_approved, update_task,
};

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[tasks] read on POST review failed", e)),
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[tasks] role read on POST review failed", e)),
    };
    if !can_edit(role.as_deref()) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let status = match talaria_body::enum_member(obj, "status", &["approved", "rejected"]) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let notes = match optional_max_string_member(obj, "notes", 20_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let reviewer = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "reviewer".into());
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = add_review(&deps, &id, &reviewer, &status, notes.as_deref()).await {
        return Ok(internal("[tasks] review record failed", e));
    }
    let approved = status == "approved";
    // APPROVING DOES NOT MOVE THE TICKET. It marks it signed off and leaves
    // the column alone: a reviewed ticket routinely still has a merge, a
    // deploy or a release in front of it, and forcing it into `done` claimed
    // the work had shipped when it had not — with nowhere to park it that told
    // the truth. A person moves it to done when it really is done, and the
    // review queues read the mark so an approved ticket stops asking to be
    // reviewed again.
    if approved {
        if let Err(e) = set_task_approved(&state.pg, &id, true).await {
            return Ok(internal("[tasks] approval mark failed", e));
        }
        return Ok(match get_task(&state.pg, &id).await {
            Ok(Some(t2)) => Json(json!({ "task": t2 })).into_response(),
            Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
            Err(e) => internal("[tasks] read after approval failed", e),
        });
    }
    // A REJECTION is the move. Boards rename and recategorize their columns,
    // so resolve the destination from the BOARD — hardcoding 'in_progress'
    // 400s the gate on any board that renamed it. And resolve it from
    // status_meta, not from list_statuses: the reject destination is a
    // DESTINATION, and spelling `find(category == active)` here does not
    // exclude terminal columns, so on a board whose first active column is
    // labelled "Cancelled" (an off-board terminal key) "request changes"
    // would CANCEL the ticket. `active_key` is picked from the one placeable
    // list every other destination comes from.
    let meta = match status_meta(&state.pg, &task.board_id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[tasks] status meta on POST review failed", e)),
    };
    let target = meta
        .active_key
        .clone()
        .or_else(|| meta.assigned_key.clone())
        .filter(|t| meta.keys.contains(t));
    let Some(target) = target else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "this board has no working column to move the ticket into",
        ));
    };
    // Sending it back for more work retires any earlier sign-off; update_task
    // carries `approved_at` through a move, so clear it explicitly.
    if let Err(e) = set_task_approved(&state.pg, &id, false).await {
        return Ok(internal("[tasks] approval clear failed", e));
    }
    // Any update_task throw here is the house 500, never a refusal shape.
    let patch = TaskPatch {
        status: Some(target),
        ..Default::default()
    };
    Ok(
        match update_task(&deps, &id, patch, &TaskActor::human(reviewer)).await {
            Ok(t2) => Json(json!({ "task": t2 })).into_response(),
            Err(e) => internal("[tasks] review move failed", e.message()),
        },
    )
}

// /api/tasks/{id}/review — port of ui/src/routes/api/tasks.$id.review.ts.
// The human quality gate. Approve moves the ticket to the board's done
// column; reject sends it back to the board's first working column. Board
// owner/editor only.

use crate::boards::{board_role, can_edit};
use crate::body::{as_object, optional_max_string_member, parse};
use crate::error::house_error;
use crate::session::require_user;
use crate::state::AppState;
use crate::statuses::status_meta;
use crate::tasks::{TaskActor, TaskDeps, TaskPatch, add_review, get_task, update_task};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

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
    if let Some(gate) = crate::params::uuid_gate("tasks", "POST review", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST review failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[tasks] role read on POST review failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if !can_edit(role.as_deref()) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let status = match crate::body::enum_member(obj, "status", &["approved", "rejected"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let notes = match optional_max_string_member(obj, "notes", 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let reviewer = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "reviewer".into());
    let deps = TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = add_review(&deps, &id, &reviewer, &status, notes.as_deref()).await {
        tracing::error!("[tasks] review record failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    // Boards rename and recategorize their columns, so resolve the target
    // from the BOARD — hardcoding 'done'/'in_progress' 400s human sign-off
    // on any board that renamed them. And resolve it from status_meta, not
    // from list_statuses: the reject destination is a DESTINATION, and
    // spelling `find(category == active)` here does not exclude terminal
    // columns, so on a board whose first active column is labelled
    // "Cancelled" (an off-board terminal key) "request changes" would
    // CANCEL the ticket. `active_key` is picked from the one placeable list
    // every other destination comes from. `done_keys[0]` stays the approve
    // target on purpose: sign-off is the one move whose destination IS
    // terminal, it is a person's call, and the membership check below
    // catches the legacy fallback.
    let meta = match status_meta(&state.pg, &task.board_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[tasks] status meta on POST review failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let approved = status == "approved";
    let target = if approved {
        // done_keys falls back to ['done'], so entry zero always exists.
        Some(meta.done_keys[0].clone())
    } else {
        meta.active_key
            .clone()
            .or_else(|| meta.assigned_key.clone())
    };
    let Some(target) = target else {
        return house_error(
            StatusCode::BAD_REQUEST,
            if approved {
                "this board has no done column to move the ticket into"
            } else {
                "this board has no working column to move the ticket into"
            },
        );
    };
    if !meta.keys.contains(&target) {
        return house_error(
            StatusCode::BAD_REQUEST,
            if approved {
                "this board has no done column to move the ticket into"
            } else {
                "this board has no working column to move the ticket into"
            },
        );
    }
    // TS has no catch here — any update_task throw is an unhandled 500 on
    // that side, so every error maps to the house 500, not a refusal shape.
    let patch = TaskPatch {
        status: Some(target),
        ..Default::default()
    };
    match update_task(&deps, &id, patch, &TaskActor::human(reviewer)).await {
        Ok(t2) => Json(json!({ "task": t2 })).into_response(),
        Err(e) => {
            tracing::error!("[tasks] review move failed: {:?}", e.message());
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

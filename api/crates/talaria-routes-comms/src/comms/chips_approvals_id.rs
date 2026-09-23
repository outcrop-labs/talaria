// POST /api/chat/chips/approvals/{id} — approve or deny a chip.
// Google sends execute through the existing confirm-send. Ticket moves apply
// as the person who approved. Either way the chip that follows names the
// tools that approval unlocked.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_api_facades::google::pending_actions::decide_action;
use talaria_body::{enum_member, parse};
use talaria_chips::{
    load_approval, mark_approval, merge_unlocked, surface_for_agent, tools_unlocked, unlock_chip,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_session::secretbox_or_500;
use talaria_state::AppState;
use talaria_tasks::{TaskActor, TaskDeps, TaskError, TaskPatch, update_task};

// doc: Approve or deny a protected action surfaced as a chip in the thread.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let decision = match enum_member(obj, "decision", &["approve", "reject"]) {
        Ok(d) => d,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let row = match load_approval(&state.pg, &id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[chips] approval read failed", e)),
    };
    if row.status != "pending" {
        return Ok(Json(json!({ "status": row.status, "tools": [] })).into_response());
    }
    let allowed = row.owner_user_id.as_deref() == Some(user.id.as_str()) || user.role == "admin";
    if !allowed {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if decision == "reject" {
        if let Err(e) = mark_approval(&state.pg, &row.id, "denied", &user.id).await {
            return Ok(internal("[chips] deny failed", e));
        }
        return Ok(Json(json!({ "status": "denied", "tools": [] })).into_response());
    }

    if let Err(response) = execute(&state, &user.id, user.role == "admin", &row).await {
        return Ok(response);
    }
    if let Err(e) = mark_approval(&state.pg, &row.id, "approved", &user.id).await {
        return Ok(internal("[chips] approve mark failed", e));
    }
    let tools = tools_unlocked(&row.kind);
    if let Some(conversation_id) = &row.conversation_id
        && let Err(e) = merge_unlocked(&state.pg, conversation_id, tools).await
    {
        return Ok(internal("[chips] unlock failed", e));
    }
    if let Some(agent) = &row.agent_model {
        let chip = unlock_chip(&row.id, &row.kind);
        let redis = state.redis().await.ok();
        if let Err(e) = surface_for_agent(&state.pg, redis, agent, vec![chip]).await {
            return Ok(internal("[chips] unlock chip failed", e));
        }
    }
    Ok(Json(json!({ "status": "approved", "tools": tools })).into_response())
}

async fn execute(
    state: &AppState,
    user_id: &str,
    is_admin: bool,
    row: &talaria_chips::ApprovalRow,
) -> Result<(), Response> {
    match row.kind.as_str() {
        "gmail_send" | "calendar_create" => {
            let external = row.external_id.as_deref().unwrap_or("");
            if external.is_empty() {
                return Err(house_error(StatusCode::BAD_REQUEST, "nothing to send"));
            }
            let sb = secretbox_or_500(state, "[chips] confirm-send").await?;
            match decide_action(
                &state.pg,
                &sb,
                external,
                user_id,
                is_admin,
                "approve",
                now_ms(),
            )
            .await
            {
                Ok(Some(outcome)) if outcome.status == "executed" => Ok(()),
                Ok(Some(outcome)) if outcome.status == "forbidden" => {
                    Err(house_error(StatusCode::FORBIDDEN, "forbidden"))
                }
                Ok(Some(outcome)) => Err(house_error(
                    StatusCode::BAD_GATEWAY,
                    outcome
                        .message
                        .as_deref()
                        .unwrap_or("the action did not send"),
                )),
                Ok(None) => Err(house_error(StatusCode::NOT_FOUND, "not found")),
                Err(e) => Err(internal("[chips] confirm-send failed", e)),
            }
        }
        "ticket_move" => {
            let task_id = row
                .payload
                .get("taskId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if task_id.is_empty() {
                return Err(house_error(
                    StatusCode::BAD_REQUEST,
                    "no ticket on this approval",
                ));
            }
            let text = |key: &str| {
                row.payload
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            let patch = TaskPatch {
                title: text("title"),
                status: text("status"),
                priority: text("priority"),
                description: text("description").map(Some),
                ..TaskPatch::default()
            };
            let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
            match update_task(&deps, task_id, patch, &TaskActor::human(user_id)).await {
                Ok(_) => Ok(()),
                Err(TaskError::ApprovalRequired(msg) | TaskError::Refusal(msg)) => {
                    Err(house_error(StatusCode::BAD_REQUEST, &msg))
                }
                Err(TaskError::Db(e)) => Err(internal("[chips] ticket move failed", e)),
            }
        }
        _ => Ok(()),
    }
}

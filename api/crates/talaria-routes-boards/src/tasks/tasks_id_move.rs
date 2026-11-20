// POST /api/tasks/{id}/move { boardId, status?, preview?, conversationId? }.
// Move a ticket to another board — an ACTION, so POST (API-CONVENTIONS
// "Methods"). `conversationId` is an agent's claim that a person asked for a
// move out of review: the thread they asked in, proven by `move_task`
// (talaria-tasks `asked`) and ignored for any other move or caller.
//
// The decisions live in talaria-tasks `move_task` (permission on both
// boards, the live holds, the status/label/assignee translation, the
// re-key); this route authenticates, reads the body, and puts the answer
// on the wire:
//   200 { task, moved }            the ticket on its new board, and what changed
//   200 { preview: true, moved }   `preview: true` — what WOULD change; nothing written
//   400 the caller's fix · 403 the mover may not · 404 no such ticket ·
//   409 a work session, a workchain or sub-tasks hold it where it is.
//
// AUTHORIZATION AFTER THE BODY, on purpose (API-CONVENTIONS): who may move
// depends on the DESTINATION board, which only the body names. The session
// or agent credential is still checked first.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_body::{
    optional_boolean_member, optional_string_member, optional_uuid_member, parse, uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_tasks::{AskClaim, MoveError, MoveRequest, Mover, TaskDeps, move_task};

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let caller = agent_caller(&state.pg, &headers).await?;
    // Only the caller's own key can cite a person's ask; the legacy shared
    // key only asserts which agent it is.
    let proven_agent = caller.as_ref().map(|c| !c.legacy && c.id.is_some());
    let mover = match caller {
        Some(caller) => {
            let model = caller.model.clone();
            Mover::Agent {
                subject: AgentSubject::Caller(caller),
                model,
            }
        }
        None => {
            let user = require_user(&state, &headers).await?;
            let actor = user
                .email
                .clone()
                .or_else(|| user.name.clone())
                .unwrap_or_else(|| "user".into());
            Mover::Human {
                user_id: user.id.clone(),
                actor,
            }
        }
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let board_id = match uuid_member(obj, "boardId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let status = match optional_string_member(obj, "status", 40) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let preview = match optional_boolean_member(obj, "preview") {
        Ok(v) => v.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Agents: the conversation or channel a person asked for this move out of
    // review in. A claim move_task proves (talaria-tasks `asked`).
    let conversation_id = match optional_uuid_member(obj, "conversationId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let ask = match (proven_agent, conversation_id) {
        (Some(proven_caller), Some(thread_id)) => Some(AskClaim {
            thread_id,
            proven_caller,
        }),
        _ => None,
    };
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    // The write and what it owes people run where a caller hanging up cannot
    // cancel them — the same reason PUT /api/tasks/{id} spawns (C30).
    let task_id = id.clone();
    let write = tokio::spawn(async move {
        let req = MoveRequest {
            board_id: &board_id,
            status: status.as_deref(),
            preview,
            ask: ask.as_ref(),
        };
        move_task(&deps, &task_id, &req, &mover).await
    });
    let outcome = match write.await {
        Ok(Ok(o)) => o,
        Ok(Err(MoveError::NotFound)) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Ok(Err(MoveError::Bad(msg))) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        Ok(Err(MoveError::Forbidden(msg))) => {
            return Ok(house_error(StatusCode::FORBIDDEN, &msg));
        }
        Ok(Err(MoveError::Conflict(msg))) => return Ok(house_error(StatusCode::CONFLICT, &msg)),
        Ok(Err(MoveError::Db(e))) => return Ok(internal("[tasks] move failed", e)),
        Err(e) => return Ok(internal("[tasks] move task failed", e)),
    };
    if outcome.moved.preview {
        return Ok(Json(json!({ "preview": true, "moved": outcome.moved })).into_response());
    }
    // The activity line and the audit row (`task.board_move`) were written in
    // the move's own transaction: a committed move always has both.
    Ok(Json(json!({ "task": outcome.task, "moved": outcome.moved })).into_response())
}

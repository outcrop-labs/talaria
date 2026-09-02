// /api/tasks/{id}/comments. GET → the thread (board member or board-allowed
// agent). POST → add a comment; an agent goes through the one
// agent-authority predicate with intent Comment — board policy plus
// archival, and deliberately NOT the closed-status clause, because
// commenting is the agent's channel on work it can no longer edit.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::boards::{board_allows_agent, board_role, list_members};
use crate::body::{as_object, optional_uuid_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::mentions::{Mentionee, notify_mentions};
use crate::notify::NotifyDeps;
use crate::session::{get_session_user, require_user};
use crate::state::AppState;
use crate::tasks::{
    AgentIntent, AgentWriteTarget, TaskDeps, add_comment, agent_ticket_refusal, get_task,
    list_comments,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Who may READ this ticket's comments: a board member, or an agent the
/// board's policy allows. Reading changes nothing, so it is the BOARD
/// question only — an agent that can see the board can read the thread on a
/// ticket that is closed or archived. Returns the reader's audit name.
async fn comment_reader(
    state: &AppState,
    headers: &HeaderMap,
    board_id: &str,
) -> Result<String, Response> {
    let caller = agent_caller(&state.pg, headers).await?;
    if let Some(caller) = caller {
        // The CALLER, not its model: the elevated-assistant bypass inside
        // board policy is org-wide reach, and a legacy caller only asserted
        // its name.
        let allowed =
            match board_allows_agent(&state.pg, board_id, &AgentSubject::Caller(caller.clone()))
                .await
            {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!("[tasks] agent policy read on GET comments failed: {e}");
                    return Err(thrown_internal_error());
                }
            };
        if !allowed {
            return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        return Ok(caller.model);
    }
    let user = require_user(state, headers).await?;
    match board_role(&state.pg, &user.id, board_id).await {
        Ok(Some(_)) => Ok(user
            .email
            .clone()
            .or_else(|| user.name.clone())
            .unwrap_or_else(|| "user".into())),
        Ok(None) => Err(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[tasks] role read on GET comments failed: {e}");
            Err(thrown_internal_error())
        }
    }
}

/// Who may POST a comment. Posting is an ACT on the ticket, so an agent goes
/// through agent_ticket_refusal with intent Comment — where the
/// archived-board / archived-ticket question is decided once. A human needs
/// only membership: commenting is the weakest act on a ticket.
async fn comment_author(
    state: &AppState,
    headers: &HeaderMap,
    task: &crate::tasks::Task,
) -> Result<String, Response> {
    let caller = agent_caller(&state.pg, headers).await?;
    if let Some(caller) = caller {
        let target = AgentWriteTarget {
            board_id: task.board_id.clone(),
            status: task.status.clone(),
            archived_at: task.archived_at.clone(),
        };
        match agent_ticket_refusal(
            &state.pg,
            &target,
            &AgentSubject::Caller(caller.clone()),
            AgentIntent::Comment,
        )
        .await
        {
            Ok(None) => return Ok(caller.model),
            Ok(Some(shut)) => return Err(house_error(StatusCode::FORBIDDEN, &shut)),
            Err(e) => {
                tracing::error!("[tasks] agent authority on POST comment failed: {e}");
                return Err(thrown_internal_error());
            }
        }
    }
    let user = require_user(state, headers).await?;
    match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(Some(_)) => Ok(user
            .email
            .clone()
            .or_else(|| user.name.clone())
            .unwrap_or_else(|| "user".into())),
        Ok(None) => Err(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[tasks] role read on POST comment failed: {e}");
            Err(thrown_internal_error())
        }
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("tasks", "GET comments", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on GET comments failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Err(gate) = comment_reader(&state, &headers, &task.board_id).await {
        return gate;
    }
    match list_comments(&state.pg, &id).await {
        Ok(comments) => Json(json!({ "comments": comments })).into_response(),
        Err(e) => {
            tracing::error!("[tasks] comment list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("tasks", "POST comment", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST comment failed: {e}");
            return thrown_internal_error();
        }
    };
    let author = match comment_author(&state, &headers, &task).await {
        Ok(a) => a,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let content = match string_member(obj, "content", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent_id = match optional_uuid_member(obj, "parentId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let deps = TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
    let comment = match add_comment(&deps, &id, &author, &content, parent_id.as_deref()).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[tasks] comment add failed: {e}");
            return thrown_internal_error();
        }
    };
    // No inline index on this path — comments reach the activity brain only
    // through the backfill/reindex runs, which read the PERSISTED row. That
    // is the invariant: add_comment runs an agent's comment through the
    // agent-writes door and returns the REDACTED body; indexing the raw body
    // would put a credential into a brain read back into model contexts —
    // the exact re-entry the guardrails forbid. The persisted comment is
    // already clean; an inline index, if ever added, MUST carry
    // `comment.content`, never the parsed body's.
    //
    // @mention any board member — they get an inbox notification linking to
    // the ticket. Detached; the POST returns immediately. The mention text
    // is `comment.content` — the REDACTED body, for the same reason as the
    // index: it lands in a person's inbox and must not be the one copy of an
    // agent's comment that still carries the credential.
    let sender = get_session_user(&state, &headers).await.ok().flatten();
    let sender_id = sender.as_ref().map(|s| s.id.clone()).unwrap_or_default();
    let sender_label = sender
        .and_then(|s| s.name)
        .unwrap_or_else(|| author.clone());
    let pg = state.pg.clone();
    let notify = NotifyDeps::publishing(pg.clone(), state.redis().await.ok());
    let board_id = task.board_id.clone();
    let where_ = task.ticket_ref.clone().unwrap_or_else(|| "a ticket".into());
    let mention_content = comment.content.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        let Ok(members) = list_members(&pg, &board_id).await else {
            return;
        };
        let members: Vec<Mentionee> = members
            .into_iter()
            .map(|m| Mentionee {
                user_id: m.user_id,
                name: m.name,
                email: m.email,
            })
            .collect();
        let href = format!("/boards/{board_id}/{task_id}");
        let _ = notify_mentions(
            &notify,
            &members,
            &sender_id,
            &sender_label,
            &mention_content,
            &where_,
            &href,
        )
        .await;
    });
    Json(json!({ "comment": comment })).into_response()
}

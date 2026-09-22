// /api/tasks/{id}/dependencies. POST { dependsOnId } → this ticket is
// blocked by another. DELETE → remove. Editors or board-allowed agents may
// add (part of triage); removal is human-only. The dependency target must
// live on the same board.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_boards::{board_allows_agent, board_role, can_edit};
use talaria_body::parse;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_tasks::{
    AgentIntent, AgentWriteTarget, TaskDeps, TaskError, add_dependency, agent_ticket_refusal,
    get_task, remove_dependency,
};

fn write_target(t: &talaria_tasks::Task) -> AgentWriteTarget {
    AgentWriteTarget {
        board_id: t.board_id.clone(),
        status: t.status.clone(),
        archived_at: t.archived_at.clone(),
    }
}

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
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[tasks] read on POST dependency failed", e)),
    };
    let caller = agent_caller(&state.pg, &headers).await?;
    // The AGENT itself, not a boolean — the second check below needs the
    // same subject the first one used.
    let (actor, agent) = if let Some(caller) = caller {
        // The CALLER, not its model: board policy's elevated-assistant
        // bypass is org-wide reach, and a legacy caller only asserted its
        // name.
        let allowed = match board_allows_agent(
            &state.pg,
            &task.board_id,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                return Ok(internal(
                    "[tasks] agent policy read on POST dependency failed",
                    e,
                ));
            }
        };
        if !allowed {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        // The central agent-write invariant, imported rather than restated:
        // `add_dependency` never reaches `update_task` (it writes
        // task_dependencies + a task_activity line directly), so the same
        // predicate agent_safe_patch asks is asked here — closed, archived,
        // and archived-board, all three, from one definition.
        let target = write_target(&task);
        match agent_ticket_refusal(
            &state.pg,
            &target,
            &AgentSubject::Caller(caller.clone()),
            AgentIntent::Write,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(shut)) => return Ok(house_error(StatusCode::FORBIDDEN, &shut)),
            Err(e) => {
                return Ok(internal(
                    "[tasks] agent authority on POST dependency failed",
                    e,
                ));
            }
        }
        (caller.model.clone(), Some(caller))
    } else {
        let user = require_user(&state, &headers).await?;
        let role = match board_role(&state.pg, &user.id, &task.board_id).await {
            Ok(r) => r,
            Err(e) => return Ok(internal("[tasks] role read on POST dependency failed", e)),
        };
        if !can_edit(role.as_deref()) {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        (
            user.email
                .clone()
                .or_else(|| user.name.clone())
                .unwrap_or_else(|| "user".into()),
            None,
        )
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let depends_raw = match talaria_body::string_member(obj, "dependsOnId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let depends_on_id = match super::resolve_task_path(&state.pg, &depends_raw).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let dep = match get_task(&state.pg, &depends_on_id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "must be a ticket on this board",
            ));
        }
        Err(e) => {
            return Ok(internal(
                "[tasks] blocker read on POST dependency failed",
                e,
            ));
        }
    };
    if dep.board_id != task.board_id {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "must be a ticket on this board",
        ));
    }
    // The edge lands on BOTH tickets (it shows in the target's "blocks"
    // list), so the rule applies to the target too.
    if let Some(agent) = agent.as_ref() {
        let target = write_target(&dep);
        match agent_ticket_refusal(
            &state.pg,
            &target,
            &AgentSubject::Caller(agent.clone()),
            AgentIntent::Write,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(dep_shut)) => {
                return Ok(house_error(
                    StatusCode::FORBIDDEN,
                    &format!("{dep_shut}. That is the ticket you named as a blocker."),
                ));
            }
            Err(e) => {
                return Ok(internal(
                    "[tasks] blocker authority on POST dependency failed",
                    e,
                ));
            }
        }
    }
    // `add_dependency` REFUSES a cycle (X blocks Y, Y blocks X: a graph no
    // ticket in it can ever satisfy) — a write that needs a person is 403, a
    // request that cannot be satisfied is 400, and both carry the sentence
    // that says why.
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    Ok(
        match add_dependency(&deps, &id, &depends_on_id, &actor).await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(TaskError::ApprovalRequired(msg)) => house_error(StatusCode::FORBIDDEN, &msg),
            Err(TaskError::Refusal(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
            Err(TaskError::Db(e)) => internal("[tasks] dependency add failed", e),
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
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    // One 403 for both a missing ticket and a role failure — the dependency
    // plane does not reveal whether the id exists.
    let task = match get_task(&state.pg, &id).await {
        Ok(t) => t,
        Err(e) => return Ok(internal("[tasks] read on DELETE dependency failed", e)),
    };
    let editable = match task.as_ref() {
        Some(t) => match board_role(&state.pg, &user.id, &t.board_id).await {
            Ok(r) => can_edit(r.as_deref()),
            Err(e) => return Ok(internal("[tasks] role read on DELETE dependency failed", e)),
        },
        None => false,
    };
    if !editable {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let depends_raw = match talaria_body::string_member(obj, "dependsOnId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let depends_on_id = match super::resolve_task_path(&state.pg, &depends_raw).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    Ok(match remove_dependency(&deps, &id, &depends_on_id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => internal("[tasks] dependency remove failed", e),
    })
}

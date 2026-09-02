// /api/tasks/{id}/dependencies. POST { dependsOnId } → this ticket is
// blocked by another. DELETE → remove. Editors or board-allowed agents may
// add (part of triage); removal is human-only. The dependency target must
// live on the same board.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::boards::{board_allows_agent, board_role, can_edit};
use crate::body::{as_object, parse, uuid_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use crate::tasks::{
    AgentIntent, AgentWriteTarget, TaskDeps, TaskError, add_dependency, agent_ticket_refusal,
    get_task, remove_dependency,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

fn write_target(t: &crate::tasks::Task) -> AgentWriteTarget {
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
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("tasks", "POST dependency", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST dependency failed: {e}");
            return thrown_internal_error();
        }
    };
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
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
                tracing::error!("[tasks] agent policy read on POST dependency failed: {e}");
                return thrown_internal_error();
            }
        };
        if !allowed {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
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
            Ok(Some(shut)) => return house_error(StatusCode::FORBIDDEN, &shut),
            Err(e) => {
                tracing::error!("[tasks] agent authority on POST dependency failed: {e}");
                return thrown_internal_error();
            }
        }
        (caller.model.clone(), Some(caller))
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        let role = match board_role(&state.pg, &user.id, &task.board_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[tasks] role read on POST dependency failed: {e}");
                return thrown_internal_error();
            }
        };
        if !can_edit(role.as_deref()) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
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
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let depends_on_id = match uuid_member(obj, "dependsOnId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let dep = match get_task(&state.pg, &depends_on_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return house_error(StatusCode::BAD_REQUEST, "must be a ticket on this board"),
        Err(e) => {
            tracing::error!("[tasks] blocker read on POST dependency failed: {e}");
            return thrown_internal_error();
        }
    };
    if dep.board_id != task.board_id {
        return house_error(StatusCode::BAD_REQUEST, "must be a ticket on this board");
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
                return house_error(
                    StatusCode::FORBIDDEN,
                    &format!("{dep_shut}. That is the ticket you named as a blocker."),
                );
            }
            Err(e) => {
                tracing::error!("[tasks] blocker authority on POST dependency failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    // `add_dependency` REFUSES a cycle (X blocks Y, Y blocks X: a graph no
    // ticket in it can ever satisfy) — a write that needs a person is 403, a
    // request that cannot be satisfied is 400, and both carry the sentence
    // that says why.
    let deps = TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
    match add_dependency(&deps, &id, &depends_on_id, &actor).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(TaskError::ApprovalRequired(msg)) => house_error(StatusCode::FORBIDDEN, &msg),
        Err(TaskError::Refusal(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
        Err(TaskError::Db(e)) => {
            tracing::error!("[tasks] dependency add failed: {e}");
            thrown_internal_error()
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
    if let Some(gate) = crate::params::uuid_gate("tasks", "DELETE dependency", &id) {
        return gate;
    }
    // One 403 for both a missing ticket and a role failure — the dependency
    // plane does not reveal whether the id exists.
    let task = match get_task(&state.pg, &id).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[tasks] read on DELETE dependency failed: {e}");
            return thrown_internal_error();
        }
    };
    let editable = match task.as_ref() {
        Some(t) => match board_role(&state.pg, &user.id, &t.board_id).await {
            Ok(r) => can_edit(r.as_deref()),
            Err(e) => {
                tracing::error!("[tasks] role read on DELETE dependency failed: {e}");
                return thrown_internal_error();
            }
        },
        None => false,
    };
    if !editable {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let depends_on_id = match uuid_member(obj, "dependsOnId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let deps = TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
    match remove_dependency(&deps, &id, &depends_on_id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => {
            tracing::error!("[tasks] dependency remove failed: {e}");
            thrown_internal_error()
        }
    }
}

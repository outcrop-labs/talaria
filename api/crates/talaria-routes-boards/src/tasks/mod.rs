use axum::http::StatusCode;
use axum::response::Response;
use talaria_error::{house_error, internal};
use talaria_tasks::{ResolvedTaskId, resolve_task_id};

/// The address a tool (or a person) passed for a ticket: the row uuid, or
/// the ref the board shows (`PLAT-118`). Missing is 404 — a non-uuid used to
/// be a 500 from `uuid_gate` before the access check, so an agent who had
/// the ticket could not open it. Ambiguous refs name the collision instead
/// of picking a board.
pub async fn resolve_task_path(pg: &sqlx::PgPool, raw: &str) -> Result<String, Response> {
    match resolve_task_id(pg, raw).await {
        Ok(ResolvedTaskId::One(id)) => Ok(id),
        Ok(ResolvedTaskId::Missing) => Err(house_error(StatusCode::NOT_FOUND, "not found")),
        Ok(ResolvedTaskId::Ambiguous) => Err(house_error(
            StatusCode::CONFLICT,
            "that ticket ref matches more than one board — pass the ticket id",
        )),
        Err(e) => Err(internal("[tasks] ticket lookup failed", e)),
    }
}

// Tasks, comments, dependencies, review, usage, watchers; workflows.
pub mod tasks_id;
pub mod tasks_id_channel;
pub mod tasks_id_comments;
pub mod tasks_id_dependencies;
pub mod tasks_id_review;
pub mod tasks_id_usage;
pub mod tasks_id_watchers;
pub mod tasks_id_work_session;
pub mod tasks_id_work_session_stop;
pub mod tasks_id_work_sessions;
pub mod workflows;
pub mod workflows_id;

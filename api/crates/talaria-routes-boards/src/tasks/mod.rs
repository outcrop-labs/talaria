use axum::http::StatusCode;
use axum::response::Response;
use talaria_error::{house_error, internal};
use talaria_tasks::{ResolvedTaskId, resolve_task_id};

/// The one sentence a caller can act on, spelled once. It teaches BOTH
/// accepted shapes: the row uuid, or the ref the board shows (`PLAT-118`).
pub const TASK_ID_BAD: &str = "task id must be a UUID or a ticket ref like TALA-30";

/// The address a tool (or a person) passed for a ticket: the row uuid, or
/// the ref the board shows (`PLAT-118`). Malformed — neither shape — is a
/// 400 teaching the caller the fix, before anything touches the database:
/// a non-uuid used to be a 500 from `uuid_gate` before the access check, so
/// an agent who HAD the ticket could not open it. A well-shaped address with
/// no row behind it is 404 and NAMES the address, because "not found" on a
/// ref the caller copied off a card is not actionable — knowing WHICH string
/// found nothing is. Ambiguous refs name the collision instead of picking a
/// board. Neither the 404 nor the 400 reveals existence: both run before the
/// access check, and an unknown address answers the same regardless of
/// session.
pub async fn resolve_task_path(pg: &sqlx::PgPool, raw: &str) -> Result<String, Response> {
    match resolve_task_id(pg, raw).await {
        Ok(ResolvedTaskId::One(id)) => Ok(id),
        Ok(ResolvedTaskId::Malformed) => Err(house_error(StatusCode::BAD_REQUEST, TASK_ID_BAD)),
        Ok(ResolvedTaskId::Missing) => Err(house_error(
            StatusCode::NOT_FOUND,
            &format!("no ticket matches {raw}"),
        )),
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

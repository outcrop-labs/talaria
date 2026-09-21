// Boards, their members/labels/statuses/views/workchains, per-board agents
// and events.
pub mod boards;
pub mod boards_id;
pub mod boards_id_agent_requests;
pub mod boards_id_agents;
pub mod boards_id_events;
pub mod boards_id_labels;
pub mod boards_id_members;
pub mod boards_id_statuses;
pub mod boards_id_tasks;
pub mod boards_id_templates;
pub mod boards_id_views;
pub mod boards_id_work_sessions;
pub mod boards_id_workchains;

use axum::http::StatusCode;
use axum::response::Response;
use talaria_boards::{board_role, can_edit};
use talaria_error::{house_error, internal};
use talaria_state::AppState;

/// Owner/editor. A non-member is refused the same way `Ok(None)` is: a board
/// they cannot see must not be distinguishable from one that does not exist.
pub(crate) async fn edit_gate(
    state: &AppState,
    user_id: &str,
    id: &str,
    action: &str,
) -> Option<Response> {
    match board_role(&state.pg, user_id, id).await {
        Ok(role) if can_edit(role.as_deref()) => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => Some(internal(
            &format!("[boards] role read on {action} failed"),
            e,
        )),
    }
}

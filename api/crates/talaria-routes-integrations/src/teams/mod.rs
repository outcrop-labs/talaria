// Teams, members, agents, and admin access.
pub mod teams;
pub mod teams_directory;
pub mod teams_id;
pub mod teams_id_access;
pub mod teams_id_agents;
pub mod teams_id_members;

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use talaria_error::{house_error, internal};
use talaria_session::require_view;
use talaria_state::AppState;
use talaria_teams::team_role;

pub(crate) async fn reader_gate(
    state: &AppState,
    headers: &HeaderMap,
    user_id: &str,
    team_id: &str,
    action: &str,
) -> Option<Response> {
    match team_role(&state.pg, user_id, team_id).await {
        Ok(Some(_)) => None,
        Ok(None) => require_view(state, headers, "/teams").await.err(),
        Err(e) => Some(internal(
            &format!("[teams] role read on {action} failed"),
            e,
        )),
    }
}

/// Owner-only. Same three-line read as reader_gate above, same refusal.
pub(crate) async fn owner_gate(
    state: &AppState,
    user_id: &str,
    team_id: &str,
    action: &str,
) -> Option<Response> {
    match team_role(&state.pg, user_id, team_id).await {
        Ok(Some(role)) if role == "owner" => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => Some(internal(
            &format!("[teams] role read on {action} failed"),
            e,
        )),
    }
}

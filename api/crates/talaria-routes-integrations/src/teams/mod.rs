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

/// Who a membership write re-renders and rolls: the agents named (a
/// `team_agents` change — a team's MCP grants ride on that table, so the
/// agent's rendered server set changed), or one person's personal assistant
/// (a `team_members` change — a PA's rendered set skips the servers its
/// owner's teams deny, `servers_for_agent`).
pub(crate) enum Rolls {
    Models(Vec<String>),
    User(String),
}

/// Config truth first (the render), then the live cutover (the roll) — the
/// same two strokes `me/mcp` makes when a person's connection changes. A
/// running Hermes wires its MCP servers at start, so without the roll the
/// container keeps the old server set while the registry says otherwise.
/// Detached, never the caller's problem: the membership row is already
/// written, and a roll that fails lands in the audit trail.
pub(crate) fn render_and_roll(state: &AppState, rolls: Rolls) {
    let (pg, state) = (state.pg.clone(), state.clone());
    tokio::spawn(async move {
        let sb = match state.secretbox().await {
            Ok(sb) => sb,
            Err(e) => {
                tracing::warn!("[teams] membership stored but the roll could not start: {e}");
                return;
            }
        };
        let _ = talaria_api_facades::fleet::render::render_fleet(&pg, &sb, None).await;
        match rolls {
            Rolls::Models(models) => {
                for model in &models {
                    talaria_api_facades::mcp::apply::roll_agent_for_model(&pg, &sb, model).await;
                }
            }
            Rolls::User(user_id) => {
                talaria_api_facades::mcp::apply::roll_agent_for_user(&pg, &sb, &user_id).await;
            }
        }
    });
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

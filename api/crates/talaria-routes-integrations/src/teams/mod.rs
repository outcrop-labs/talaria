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

/// Who a membership write re-renders and applies to: the agents named (a
/// `team_agents` change — a team's MCP grants ride on that table, so the
/// agent's rendered server set changed), or one person's personal assistant
/// (a `team_members` change — a PA's rendered set skips the servers its
/// owner's teams deny, `servers_for_agent`).
pub(crate) enum Applies {
    Models(Vec<String>),
    User(String),
}

/// Config truth first (the render), then the change applied to the running
/// agents it touches — the same strokes `me/mcp` makes when a person's
/// connection changes. A server a team grant adds or removes is a name in
/// the agent's config, which a running Hermes reconciles within a minute of
/// the rewrite; a tool subset that changes under a server it keeps reaches
/// it as the gateway's list-changed notice on its next keepalive
/// (`bump_tool_lists`, for `servers` — the team's granted servers, the only
/// ones a membership can move a list on). Neither restarts the agent unless
/// the render moved its container for some other reason (talaria-mcp-apply).
/// Detached, never the caller's problem: the membership row is already
/// written, and an apply that fails lands in the audit trail.
pub(crate) fn render_and_apply(state: &AppState, servers: Vec<String>, applies: Applies) {
    let (pg, state) = (state.pg.clone(), state.clone());
    tokio::spawn(async move {
        talaria_api_facades::mcp::apply::bump_tool_lists(&pg, &servers).await;
        let sb = match state.secretbox().await {
            Ok(sb) => sb,
            Err(e) => {
                tracing::warn!("[teams] membership stored but it could not be applied: {e}");
                return;
            }
        };
        let _ = talaria_api_facades::fleet::render::render_fleet(&pg, &sb).await;
        match applies {
            Applies::Models(models) => {
                for model in &models {
                    talaria_api_facades::mcp::apply::apply_agent_for_model(&pg, &sb, model).await;
                }
            }
            Applies::User(user_id) => {
                talaria_api_facades::mcp::apply::apply_agent_for_user(&pg, &sb, &user_id).await;
            }
        }
    });
}

/// The servers a team carries a grant row for (allowed or denied) — the
/// servers a change to the team's members or agents can move a tool list on.
/// Read before a write that cascades the rows away (a team delete). A read
/// that fails is logged and counts nothing: the agents keep their lists until
/// they reconnect.
pub(crate) async fn team_servers(state: &AppState, team_id: &str) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "select s.name from mcp_team_access t join mcp_servers s on s.id = t.server_id \
         where t.team_id = $1::uuid order by s.name",
    )
    .bind(team_id)
    .fetch_all(&state.pg)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(
            "[teams] the team's MCP servers could not be read ({e}) — no tool-list notice"
        );
        Vec::new()
    })
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

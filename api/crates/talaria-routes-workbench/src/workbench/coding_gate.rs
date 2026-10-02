// Who may see and change an agent's coding accounts.
//
// AN AGENT IS SOMEBODY'S. Signing an agent in to a coding subscription is a
// change to that agent, so the gate is the agent's own manager ACL —
// `require_agent_manager`, the same gate that guards its soul, its secrets and
// its lifecycle. It is deliberately NOT `agents.manage`: that permission says
// "may run the fleet", and a fleet-wide holder who does not manage this
// particular agent has no business putting a subscription behind it. Admins
// reach every agent, and a personal assistant's owner manages their own, both
// of which `manages_agent` already answers — so this file asks one question
// and gets one answer rather than inventing a second.
//
// The feature gate is separate and comes first: while an admin has not turned
// coding accounts on, every one of these routes answers 404 rather than 403.
// A feature nobody enabled should look absent, not forbidden.

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use talaria_coding_accounts as ca;
use talaria_error::house_error;
use talaria_session::{SessionUser, require_agent_manager, require_user};
use talaria_state::AppState;

/// The session user, once the feature is on. 404 while it is off.
pub async fn require_feature(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<SessionUser, Response> {
    let user = require_user(state, headers).await?;
    if !ca::enabled(&state.pg).await {
        return Err(house_error(
            StatusCode::NOT_FOUND,
            "coding accounts are not enabled on this instance",
        ));
    }
    Ok(user)
}

/// The session user who manages this agent, or a ready-to-return error.
pub async fn require_agent_editor(
    state: &AppState,
    headers: &HeaderMap,
    agent_id: &str,
) -> Result<SessionUser, Response> {
    // Feature first, so a disabled feature reads as absent for everyone rather
    // than as "forbidden" for the people who could not have used it anyway.
    require_feature(state, headers).await?;
    require_agent_manager(state, headers, agent_id).await
}

/// Whether the agent exists at all — checked after the gate, so a probe
/// cannot use this route to enumerate agent ids.
pub async fn agent_exists(state: &AppState, agent_id: &str) -> bool {
    sqlx::query_scalar::<_, i32>("select 1 from agent_defs where id = $1::uuid")
        .bind(agent_id)
        .fetch_optional(&state.pg)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false)
}

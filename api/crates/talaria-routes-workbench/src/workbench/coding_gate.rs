// Who may see and change an agent's coding accounts.
//
// "Agents they can edit" is two things in this product, and both belong here
// so no route invents a third: the `agents.manage` permission (org agents),
// or owning the agent (a personal assistant its owner configures without an
// admin role). The same pair gates the agent's own secrets panel, which is the
// closest existing analogue — these credentials are at least as sensitive.
//
// The feature gate is separate and comes first: while an admin has not turned
// coding accounts on, every one of these routes answers 404 rather than 403.
// A feature nobody enabled should look absent, not forbidden.

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use talaria_coding_accounts as ca;
use talaria_error::house_error;
use talaria_personal_agent::owns_agent;
use talaria_session::{SessionUser, require_user};
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

/// The session user who may edit this agent, or a ready-to-return error.
pub async fn require_agent_editor(
    state: &AppState,
    headers: &HeaderMap,
    agent_id: &str,
) -> Result<SessionUser, Response> {
    let user = require_feature(state, headers).await?;
    let allowed = talaria_users::has_perm(&state.pg, &user.id, &user.role, "agents.manage")
        .await
        .unwrap_or(false)
        || owns_agent(&state.pg, &user.id, None, Some(agent_id)).await;
    if !allowed {
        return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(user)
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

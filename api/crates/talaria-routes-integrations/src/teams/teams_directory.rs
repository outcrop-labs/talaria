// /api/teams/directory. GET → every org team as id/name/counts, for share
// pickers. Any signed-in human (or identity-proxied assistant). Membership
// details stay on the member-gated routes.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::internal;
use talaria_session::{acting_user, unauthorized};
use talaria_state::AppState;
use talaria_teams::list_team_directory;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    match acting_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(unauthorized()),
        Err(gate) => return Err(gate),
    }
    Ok(match list_team_directory(&state.pg).await {
        Ok(teams) => Json(json!({ "teams": teams })).into_response(),
        Err(e) => internal("[teams] directory list failed", e),
    })
}

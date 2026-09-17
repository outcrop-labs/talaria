// /api/teams/directory. GET → every org team as id/name/counts, for share
// pickers. Any signed-in human (or identity-proxied assistant). Membership
// details stay on the member-gated routes.

use crate::error::thrown_internal_error;
use crate::session::{acting_user, unauthorized};
use crate::state::AppState;
use crate::teams::list_team_directory;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match acting_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    }
    match list_team_directory(&state.pg).await {
        Ok(teams) => Json(json!({ "teams": teams })).into_response(),
        Err(e) => {
            tracing::error!("[teams] directory list failed: {e}");
            thrown_internal_error()
        }
    }
}

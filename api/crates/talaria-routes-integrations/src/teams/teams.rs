// /api/teams. GET → the caller's teams, resolved through ACTING user (a
// personal assistant acts as its owner — the identity-proxy model);
// GET ?all=1 → every org team (Manage view). POST { name } → create
// (humans only: requireUser).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::{house_error, internal};
use talaria_session::{acting_user, require_user, require_view, unauthorized};
use talaria_state::AppState;
use talaria_teams::{create_team, list_all_teams, list_teams};

fn wants_all(uri: &Uri) -> bool {
    uri.query()
        .map(|q| {
            q.split('&')
                .any(|p| p == "all=1" || p == "all=true" || p == "all")
        })
        .unwrap_or(false)
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    if wants_all(&uri) {
        let user = require_view(&state, &headers, "/teams").await?;
        return Ok(match list_all_teams(&state.pg, &user.id).await {
            Ok(teams) => Json(json!({ "teams": teams })).into_response(),
            Err(e) => internal("[teams] list-all failed", e),
        });
    }
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(unauthorized()),
        Err(gate) => return Err(gate),
    };
    Ok(match list_teams(&state.pg, &user.id).await {
        Ok(teams) => Json(json!({ "teams": teams })).into_response(),
        Err(e) => internal("[teams] list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = match talaria_body::as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let name = match talaria_body::string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let (id, team_name, created_ms) = match create_team(&state.pg, &user.id, &name).await {
        Ok(t) => t,
        Err(e) => return Ok(internal("[teams] create failed", e)),
    };
    Ok(Json(json!({
        "team": {
            "id": id,
            "name": team_name,
            "createdAt": talaria_agent_auth::epoch_ms_to_iso(created_ms),
            "role": "owner",
            "memberCount": 1,
            "agentCount": 0
        }
    }))
    .into_response())
}

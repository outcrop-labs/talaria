// /api/teams. GET → the caller's teams, resolved through ACTING user (a
// personal assistant acts as its owner — the identity-proxy model);
// GET ?all=1 → every org team (Manage view). POST { name } → create
// (humans only: requireUser).

use crate::error::{house_error, thrown_internal_error};
use crate::session::{acting_user, require_user, require_view, unauthorized};
use crate::state::AppState;
use crate::teams::{create_team, list_all_teams, list_teams};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

fn wants_all(uri: &Uri) -> bool {
    uri.query()
        .map(|q| {
            q.split('&')
                .any(|p| p == "all=1" || p == "all=true" || p == "all")
        })
        .unwrap_or(false)
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    if wants_all(&uri) {
        let user = match require_view(&state, &headers, "/teams").await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        return match list_all_teams(&state.pg, &user.id).await {
            Ok(teams) => Json(json!({ "teams": teams })).into_response(),
            Err(e) => {
                tracing::error!("[teams] list-all failed: {e}");
                thrown_internal_error()
            }
        };
    }
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    };
    match list_teams(&state.pg, &user.id).await {
        Ok(teams) => Json(json!({ "teams": teams })).into_response(),
        Err(e) => {
            tracing::error!("[teams] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = crate::body::parse(&body);
    let obj = match crate::body::as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match crate::body::string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (id, team_name, created_ms) = match create_team(&state.pg, &user.id, &name).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[teams] create failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "team": {
            "id": id,
            "name": team_name,
            "createdAt": crate::agent_auth::epoch_ms_to_iso(created_ms),
            "role": "owner",
            "memberCount": 1,
            "agentCount": 0
        }
    }))
    .into_response()
}

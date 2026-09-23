// /api/integrations/google/agent/pending — the confirm-sends queue the
// calling agent's approver actually opens. Personal assistant → its owner's
// personal queue (admin arm off). General agent → the org arm an admin sees.
//
// This is the read `draft_email` tells the agent to call before it reports a
// draft ready. A success from the draft route that this list does not contain
// is a bug; the draft route now refuses that case, and this route is how the
// agent checks rather than trusting the sentence.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::{refuse_legacy, require_agent};
use talaria_api_facades::google::agent::resolve_agent_principal;
use talaria_api_facades::google::pending_actions::{
    PendingAction, list_for_approver, pending_wire,
};
use talaria_error::{house_error, internal};
use talaria_state::AppState;

struct ApproverQueue {
    pending: Vec<PendingAction>,
    is_org: bool,
    owner_user_id: Option<String>,
}

async fn queue(state: &AppState, headers: &HeaderMap) -> Result<ApproverQueue, Response> {
    let caller = require_agent(&state.pg, headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "confirm-sends") {
        return Err(denied);
    }
    let principal = resolve_agent_principal(&state.pg, &caller.model)
        .await
        .map_err(|e| {
            internal(
                "[integrations/google/agent/pending] principal read failed",
                e,
            )
        })?;
    let pending = list_for_approver(
        &state.pg,
        principal.owner_user_id.as_deref(),
        principal.is_org,
    )
    .await
    .map_err(|e| internal("[integrations/google/agent/pending] list failed", e))?;
    Ok(ApproverQueue {
        is_org: principal.is_org,
        owner_user_id: principal.owner_user_id,
        pending,
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let ApproverQueue {
        pending,
        is_org,
        owner_user_id,
    } = queue(&state, &headers).await?;

    Ok(Json(json!({
        "pending": pending.iter().map(pending_wire).collect::<Vec<_>>(),
        "approver": {
            "userId": owner_user_id,

            "queue": if is_org { "admin" } else { "owner" },
        },
    }))
    .into_response())
}

pub async fn get_one(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let ApproverQueue {
        pending,
        is_org,
        owner_user_id,
    } = queue(&state, &headers).await?;

    let Some(action) = pending.into_iter().find(|action| action.id == id) else {
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "not in the confirm-sends queue this agent's approver sees",
        ));
    };
    Ok(Json(json!({
        "pending": pending_wire(&action),
        "approver": {
            "userId": owner_user_id,

            "queue": if is_org { "admin" } else { "owner" },
        },
    }))
    .into_response())
}

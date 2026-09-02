// /api/research/{id} — port of ui/src/routes/api/research.$id.ts.
// GET → one run + its citation registry (owner / shared member / org runs).
// DELETE → owner/admin, cancelling the run first so the driver stops
// spending on a report nobody will open.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::error::{house_error, thrown_internal_error};
use crate::research::{delete_research_run, get_research_run, research_role};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("research", "GET run", &id) {
        return gate;
    }
    let viewer = match agent_caller(&state.pg, &headers).await {
        Ok(Some(caller)) => {
            // Reading through the owner's eyes is owner-proxying — ask with
            // the CALLER so an asserted identity sees org runs only.
            match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller)).await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[research] owner resolve on run read failed: {e}");
                    return thrown_internal_error();
                }
            }
        }
        Ok(None) => {
            let user = match require_user(&state, &headers).await {
                Ok(u) => u,
                Err(gate) => return gate,
            };
            Some(user.id)
        }
        Err(resp) => return resp,
    };
    // No role is a 404, the same answer as a missing row — a stranger probing
    // ids learns nothing about which runs exist.
    match research_role(&state.pg, viewer.as_deref(), &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[research] role read on run read failed: {e}");
            return thrown_internal_error();
        }
    }
    match get_research_run(&state.pg, &id).await {
        Ok(Some((run, sources))) => Json(json!({ "run": run, "sources": sources })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[research] run read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("research", "DELETE run", &id) {
        return gate;
    }
    let found = match get_research_run(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[research] run read on delete failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((run, _)) = found else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    if run.owner_user_id.as_deref() != Some(user.id.as_str()) && user.role != "admin" {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    // Cancels the run FIRST, then deletes the record — see the comment on
    // `delete_research_run`. The report artifact survives either way: deleting
    // a run clears the queue entry, not the knowledge.
    if let Err(e) = delete_research_run(&state, &id).await {
        tracing::error!("[research] delete failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

// /api/boards/{id}/statuses. Board statuses (custom workflow columns). GET →
// the ordered list incl. the
// system Blocked column, with the diagnostics that explain whether agents may
// start/stop work on each (any member — the reader who cannot fix it can at
// least tell the owner why the board is stuck). POST create, PUT update/
// reorder, DELETE (tickets reassigned) — owner/editor.

use crate::boards::{board_role, can_edit};
use crate::body::{
    as_object, optional_boolean_member, optional_enum_member, optional_max_string_member,
    optional_string_member, parse, string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::realtime::RealtimeDeps;
use crate::session::require_user;
use crate::state::AppState;
use crate::statuses::{
    StatusPatch, agent_start_conflict, create_status, delete_status, list_statuses,
    reorder_statuses, status_diagnostics, update_status,
};
use crate::tasks::TaskDeps;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

const CATEGORIES: &[&str] = &["open", "active", "review", "done"];

/// The audit name for a board edit that MOVES TICKETS (deleting a populated
/// column, or recategorising a populated sign-off column). Both take an actor
/// because both land on each ticket's activity log, and they must agree on how
/// the person is named.
fn actor_of_user(email: Option<&str>, name: Option<&str>) -> String {
    email.or(name).unwrap_or("user").to_string()
}

async fn edit_gate(state: &AppState, user_id: &str, id: &str, action: &str) -> Option<Response> {
    match board_role(&state.pg, user_id, id).await {
        Ok(role) if can_edit(role.as_deref()) => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[boards] role read on {action} failed: {e}");
            Some(thrown_internal_error())
        }
    }
}

/// Cross-validation the per-field schemas cannot see. An agent-start column
/// is the queue agents pick work UP from, so it cannot also be a HUMAN GATE:
/// `review` + agentStart is a loop (the agent's own hand-off drops the ticket
/// straight back into its pickup queue), and `done` + agentStart turns
/// CLOSING a ticket into a dispatch (the move to done fires updateTask's
/// re-dispatch branch and starts a live work session on the ticket a person
/// just signed off). Either way it hands an agent a legitimate-looking write
/// into a column the assignment gate would otherwise refuse. Either flag may
/// arrive alone, on top of whatever the column already is, so the check runs
/// against the EFFECTIVE post-patch column. `status_key` None = a create,
/// which inherits createStatus's defaults.
async fn human_gate_conflict(
    state: &AppState,
    board_id: &str,
    status_key: Option<&str>,
    category: Option<&str>,
    agent_start: Option<bool>,
) -> Result<Option<String>, sqlx::Error> {
    if category.is_none() && agent_start.is_none() {
        return Ok(None);
    }
    let cur = match status_key {
        Some(key) => list_statuses(&state.pg, board_id)
            .await?
            .into_iter()
            .find(|s| s.key == key),
        None => None,
    };
    let category = category
        .or(cur.as_ref().map(|s| s.category.as_str()))
        .unwrap_or("active");
    let agent_start = agent_start
        .or(cur.as_ref().map(|s| s.agent_start))
        .unwrap_or(false);
    // The system Blocked column is the one entry with a non-workflow category;
    // it is never editable here (update_status refuses the key), and it is
    // never agent-start, so this can only see a real workflow category.
    Ok(agent_start_conflict(category, agent_start).map(str::to_string))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET statuses", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on GET statuses failed: {e}");
            return thrown_internal_error();
        }
    }
    // `diagnostics` rides along with the columns because it is a statement
    // ABOUT this column set, and the two must never be read from different
    // moments — a warning that names a column the list no longer has is
    // worse than no warning.
    let statuses = list_statuses(&state.pg, &id);
    let diagnostics = status_diagnostics(&state.pg, &id);
    match tokio::try_join!(statuses, diagnostics) {
        Ok((statuses, diagnostics)) => {
            Json(json!({ "statuses": statuses, "diagnostics": diagnostics })).into_response()
        }
        Err(e) => {
            tracing::error!("[boards] status list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "POST statuses", &id) {
        return gate;
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "POST statuses").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let label = match string_member(obj, "label", 1, 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let color = match optional_max_string_member(obj, "color", 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let category = match optional_enum_member(obj, "category", CATEGORIES) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let agent_start = match optional_boolean_member(obj, "agentStart") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The cross-validation runs BEFORE create_status materializes the
    // defaults in — a 400 with the reason beats a thrown error mid-write.
    match human_gate_conflict(&state, &id, None, category.as_deref(), agent_start).await {
        Ok(Some(conflict)) => return house_error(StatusCode::BAD_REQUEST, &conflict),
        Ok(None) => {}
        Err(e) => {
            tracing::error!("[boards] conflict precheck on POST statuses failed: {e}");
            return thrown_internal_error();
        }
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    match create_status(
        &state.pg,
        &realtime,
        &id,
        &label,
        color.as_deref(),
        category.as_deref(),
        agent_start,
    )
    .await
    {
        Ok(Ok(status)) => Json(json!({ "status": status })).into_response(),
        Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
        Err(e) => {
            tracing::error!("[boards] status create failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "PUT statuses", &id) {
        return gate;
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "PUT statuses").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The union resolves by shape: a statusKey member picks the patch arm
    // (which ignores an unknown `order` key), anything else must be the
    // reorder arm.
    if obj.contains_key("statusKey") {
        let status_key = match string_member(obj, "statusKey", 1, 40) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let label = match optional_string_member(obj, "label", 40) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let color = match optional_max_string_member(obj, "color", 20) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let category = match optional_enum_member(obj, "category", CATEGORIES) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let agent_start = match optional_boolean_member(obj, "agentStart") {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        match human_gate_conflict(
            &state,
            &id,
            Some(&status_key),
            category.as_deref(),
            agent_start,
        )
        .await
        {
            Ok(Some(conflict)) => return house_error(StatusCode::BAD_REQUEST, &conflict),
            Ok(None) => {}
            Err(e) => {
                tracing::error!("[boards] conflict precheck on PUT statuses failed: {e}");
                return thrown_internal_error();
            }
        }
        let patch = StatusPatch {
            label,
            color,
            category,
            agent_start,
            position: None,
        };
        // The actor: recategorising a populated review/done column moves its
        // tickets into a surviving column of the same category, one
        // update_task each, and the person who reshaped the column owns
        // those moves.
        let actor = actor_of_user(user.email.as_deref(), user.name.as_deref());
        let deps = TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
        match update_status(&deps, &id, &status_key, &patch, &actor).await {
            Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
            Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
            Err(e) => {
                tracing::error!("[boards] status update failed: {e}");
                thrown_internal_error()
            }
        }
    } else {
        let order = match crate::body::optional_string_array_member(obj, "order", 1, 40, 50) {
            Ok(Some(v)) => v,
            Ok(None) => {
                // when neither arm matches, the error names the first arm's
                // missing key — statusKey.
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "Invalid input: expected string, received undefined",
                );
            }
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
        match reorder_statuses(&state.pg, &realtime, &id, &order).await {
            Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
            Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
            Err(e) => {
                tracing::error!("[boards] status reorder failed: {e}");
                thrown_internal_error()
            }
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "DELETE statuses", &id) {
        return gate;
    }
    if let Some(gate) = edit_gate(&state, &user.id, &id, "DELETE statuses").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let status_key = match string_member(obj, "statusKey", 1, 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // reassignTo is required with a max only (no min): the empty string is a
    // legal value, and delete_status answers it with the pick-a-surviving
    // refusal rather than a length complaint.
    let reassign_to = match optional_max_string_member(obj, "reassignTo", 40) {
        Ok(Some(v)) => v,
        Ok(None) => {
            return house_error(
                StatusCode::BAD_REQUEST,
                "Invalid input: expected string, received undefined",
            );
        }
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The actor is threaded through because the reassignment lands on each
    // ticket's activity log — deleting a column moves work, and the person
    // who did it owns that move.
    let actor = actor_of_user(user.email.as_deref(), user.name.as_deref());
    let deps = TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
    match delete_status(&deps, &id, &status_key, &reassign_to, &actor).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(msg)) => house_error(StatusCode::BAD_REQUEST, &msg),
        Err(e) => {
            tracing::error!("[boards] status delete failed: {e}");
            thrown_internal_error()
        }
    }
}

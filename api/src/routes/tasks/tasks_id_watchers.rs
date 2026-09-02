// /api/tasks/{id}/watchers. POST { watcher } → follow.
// DELETE { watcher } → unfollow.
//
// WHO MAY DO WHAT, and why each is the role it is:
//
//   POST   subscribing YOURSELF needs any board role — a viewer can read the
//          ticket, so a viewer may follow it. Subscribing SOMEONE ELSE writes
//          to another person's inbox and is an editor's call. add_watcher
//          refuses a non-member (and says so); this route decides only who
//          may ask.
//
//   DELETE removing someone else is the editor's mirror of adding them.
//          REMOVING YOURSELF ALWAYS WORKS, with no board role of any kind.
//          That is the whole point: the person who most needs to unsubscribe
//          is the person getting mail about a board she cannot open, and the
//          only remaining exit must not be asking someone else to stop it.

use crate::boards::{board_role, can_edit};
use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{SessionUser, require_user};
use crate::state::AppState;
use crate::tasks::{WatchOutcome, add_watcher, get_task, list_watchers, remove_watcher};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Is this watcher string the CALLER? Their email is what the Watch button
/// sends (`user.email ?? user.name`), and the id forms are accepted because
/// `user:<uuid>` is how a human is named everywhere else on a ticket.
fn is_self(user: &SessionUser, watcher: &str) -> bool {
    let w = watcher.trim().to_lowercase();
    if let Some(email) = user.email.as_deref()
        && w == email.trim().to_lowercase()
    {
        return true;
    }
    let id = user.id.to_lowercase();
    w == id || w == format!("user:{id}")
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
    if let Some(gate) = crate::params::uuid_gate("tasks", "POST watcher", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST watcher failed: {e}");
            return thrown_internal_error();
        }
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[tasks] role read on POST watcher failed: {e}");
            return thrown_internal_error();
        }
    };
    if role.is_none() {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let watcher = match string_member(obj, "watcher", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if !is_self(&user, &watcher) && !can_edit(role.as_deref()) {
        return house_error(
            StatusCode::FORBIDDEN,
            "only a board editor can make someone else follow this ticket",
        );
    }
    match add_watcher(&state.pg, &id, &watcher).await {
        Ok(WatchOutcome::Added) => {}
        Ok(WatchOutcome::Refused(msg)) => return house_error(StatusCode::BAD_REQUEST, &msg),
        Err(e) => {
            tracing::error!("[tasks] watcher add failed: {e}");
            return thrown_internal_error();
        }
    }
    match list_watchers(&state.pg, &id).await {
        Ok(watchers) => Json(json!({ "watchers": watchers })).into_response(),
        Err(e) => {
            tracing::error!("[tasks] watcher list failed: {e}");
            thrown_internal_error()
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
    if let Some(gate) = crate::params::uuid_gate("tasks", "DELETE watcher", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on DELETE watcher failed: {e}");
            return thrown_internal_error();
        }
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let watcher = match string_member(obj, "watcher", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[tasks] role read on DELETE watcher failed: {e}");
            return thrown_internal_error();
        }
    };
    if !is_self(&user, &watcher) && !can_edit(role.as_deref()) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err(e) = remove_watcher(&state.pg, &id, &watcher).await {
        tracing::error!("[tasks] watcher remove failed: {e}");
        return thrown_internal_error();
    }
    // The unsubscribe hatch must not become a disclosure hatch: someone with
    // no membership gets confirmation that they are off the ticket, never
    // the list of who else is on it.
    if role.is_none() {
        return Json(json!({ "unwatched": true })).into_response();
    }
    match list_watchers(&state.pg, &id).await {
        Ok(watchers) => Json(json!({ "watchers": watchers })).into_response(),
        Err(e) => {
            tracing::error!("[tasks] watcher list failed: {e}");
            thrown_internal_error()
        }
    }
}

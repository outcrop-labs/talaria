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

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_boards::{board_role, can_edit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{SessionUser, require_user};
use talaria_state::AppState;
use talaria_tasks::{WatchOutcome, add_watcher, get_task, list_watchers, remove_watcher};

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
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[tasks] read on POST watcher failed", e)),
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[tasks] role read on POST watcher failed", e)),
    };
    if role.is_none() {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let watcher = match string_member(obj, "watcher", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if !is_self(&user, &watcher) && !can_edit(role.as_deref()) {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "only a board editor can make someone else follow this ticket",
        ));
    }
    match add_watcher(&state.pg, &id, &watcher).await {
        Ok(WatchOutcome::Added) => {}
        Ok(WatchOutcome::Refused(msg)) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        Err(e) => return Ok(internal("[tasks] watcher add failed", e)),
    }
    Ok(match list_watchers(&state.pg, &id).await {
        Ok(watchers) => Json(json!({ "watchers": watchers })).into_response(),
        Err(e) => internal("[tasks] watcher list failed", e),
    })
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[tasks] read on DELETE watcher failed", e)),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let watcher = match string_member(obj, "watcher", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[tasks] role read on DELETE watcher failed", e)),
    };
    if !is_self(&user, &watcher) && !can_edit(role.as_deref()) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if let Err(e) = remove_watcher(&state.pg, &id, &watcher).await {
        return Ok(internal("[tasks] watcher remove failed", e));
    }
    // The unsubscribe hatch must not become a disclosure hatch: someone with
    // no membership gets confirmation that they are off the ticket, never
    // the list of who else is on it.
    if role.is_none() {
        return Ok(Json(json!({ "unwatched": true })).into_response());
    }
    Ok(match list_watchers(&state.pg, &id).await {
        Ok(watchers) => Json(json!({ "watchers": watchers })).into_response(),
        Err(e) => internal("[tasks] watcher list failed", e),
    })
}

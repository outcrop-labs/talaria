// /api/admin/apps/change-request. "Request a change" on a plugin's card in
// Manage → Apps: the request becomes a ticket for the plugin's maintaining
// agent on the plugin's board (the board of the ticket that started it),
// created and assigned by the person asking. Anyone who sees Manage → Apps
// may ask, if they could file a ticket on that board themselves — an editor
// of it, or an admin.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_boards::{board_role, can_edit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_plugins::change::{REQUEST_MAX, request_change};
use talaria_session::require_view;
use talaria_state::AppState;
use talaria_tasks::TaskDeps;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_view(&state, &headers, "/apps").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let app = match string_member(obj, "app", 1, 64) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // What should change, in the person's words: the ticket's description,
    // its first line the title.
    let request = match string_member(obj, "request", 1, REQUEST_MAX) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let plugin = match talaria_plugins::plugin(&state.pg, &app).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::NOT_FOUND,
                &format!("{app} is not a plugin: no agent maintains it"),
            ));
        }
        Err(e) => {
            return Ok(internal(
                "[admin/apps/change-request] plugin read failed",
                e,
            ));
        }
    };
    if user.role != "admin" {
        let Some(board_id) = plugin.board_id.as_deref() else {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        };
        let role = match board_role(&state.pg, &user.id, board_id).await {
            Ok(r) => r,
            Err(e) => return Ok(internal("[admin/apps/change-request] role read failed", e)),
        };
        if !can_edit(role.as_deref()) {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                &format!(
                    "only an editor of the {} board can file a ticket there",
                    plugin.board_name.as_deref().unwrap_or("plugin's")
                ),
            ));
        }
    }
    let requester_name = user
        .name
        .clone()
        .or_else(|| user.email.clone())
        .unwrap_or_else(|| "a member".into());
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    Ok(
        match request_change(&deps, &plugin, &request, &user.id, &requester_name).await {
            Ok(filed) => Json(json!({
                "ok": true,
                "taskId": filed.task_id,
                "ticketRef": filed.ticket_ref,
                "boardId": filed.board_id,
            }))
            .into_response(),
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        },
    )
}

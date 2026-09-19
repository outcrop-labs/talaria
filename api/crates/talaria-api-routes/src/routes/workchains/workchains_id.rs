// /api/workchains/{id}. PATCH { name?, paused?, positions? } → rename,
// pause/unpause, reorder steps. DELETE → remove the chain (its tickets are
// untouched — the cascade fires the step rows, never the tasks; deleting a
// chain unlinks, it does not delete work).
//
// /api/workchains/{id}/steps: POST { taskId, after? } appends (or inserts
// after the named step); DELETE /steps/{taskId} removes one step without
// touching the task.
//
// The auth is the BOARD's, resolved through the workchain's board_id — the
// board-configuration routes' reader/writer split: any member reads,
// owner/editor writes. No 404 on a missed id: the boards family's grammar
// is forbidden (403), not hidden.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use talaria_boards::{board_role, can_edit};
use talaria_body::{
    NumKind, array_msg, array_too_big_msg, as_object, number_member, object_msg,
    optional_boolean_member, optional_string_member, optional_uuid_member, parse, uuid_member,
    zod_type_name,
};
use talaria_error::{house_error, thrown_internal_error};
use talaria_realtime_watch::{BoardEvent, RealtimeDeps, publish_board};
use talaria_session::require_user;
use talaria_state::AppState;

/// The workchain's board, in one read — the gate every handler below needs
/// before it may look at anything else.
async fn chain_board(state: &AppState, id: &str, action: &str) -> Result<Option<String>, Response> {
    let board: Option<(Option<String>,)> =
        match sqlx::query_as("select board_id::text from task_workchains where id = $1::uuid")
            .bind(id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[workchains] board read on {action} failed: {e}");
                return Err(thrown_internal_error());
            }
        };
    Ok(board.and_then(|(b,)| b))
}

/// The write gate: owner or editor — the same predicate the board's other
/// configuration writes use.
async fn write_gate(
    state: &AppState,
    user_id: &str,
    chain_id: &str,
    action: &str,
) -> Result<String, Response> {
    let Some(board_id) = chain_board(state, chain_id, action).await? else {
        return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
    };
    match board_role(&state.pg, user_id, &board_id).await {
        Ok(role) if can_edit(role.as_deref()) => Ok(board_id),
        Ok(_) => Err(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[workchains] role read on {action} failed: {e}");
            Err(thrown_internal_error())
        }
    }
}

/// The board bump every workchain write publishes — the same generic board
/// event a label or view change rides.
fn bump(deps: &RealtimeDeps, board_id: &str) {
    publish_board(
        deps,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
}

/// positions: [{ taskId, position }] — the reorder's full intended order.
/// Each entry's taskId is a uuid and its position an int >= 0; the caller
/// sends the order, not sparse gaps, and every named task must already be a
/// step of the chain (a miss is a 400, never a silent drop).
fn validate_positions(obj: &Map<String, Value>) -> Result<Option<Vec<(String, i32)>>, String> {
    let Some(v) = obj.get("positions") else {
        return Ok(None);
    };
    let Some(entries) = v.as_array() else {
        return Err(array_msg(zod_type_name(v)));
    };
    if entries.len() > 200 {
        return Err(array_too_big_msg(200));
    }
    let mut out: Vec<(String, i32)> = Vec::with_capacity(entries.len());
    for e in entries {
        let Some(o) = e.as_object() else {
            return Err(object_msg(zod_type_name(e)));
        };
        let task_id = uuid_member(o, "taskId")?;
        let pos = number_member(o, "position", NumKind::Int, 0.0, 1e9)?;
        out.push((task_id, pos as i32));
    }
    Ok(Some(out))
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("workchains", "PATCH", &id) {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_string_member(obj, "name", 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let paused = match optional_boolean_member(obj, "paused") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let positions = match validate_positions(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let board_id = match write_gate(&state, &user.id, &id, "PATCH").await {
        Ok(b) => b,
        Err(gate) => return gate,
    };
    // Nothing was asked for: ok, no write, no bump — the empty patch is the
    // workflows family's own quiet shape.
    if name.is_none() && paused.is_none() && positions.is_none() {
        return Json(json!({ "ok": true })).into_response();
    }
    if let Some(name) = &name
        && let Err(e) = sqlx::query(
            "update task_workchains set name = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(name)
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[workchains] rename failed: {e}");
        return thrown_internal_error();
    }
    if let Some(paused) = paused
        && let Err(e) = sqlx::query(
            "update task_workchains set paused = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(paused)
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[workchains] pause write failed: {e}");
        return thrown_internal_error();
    }
    if let Some(order) = &positions
        && let Err(msg) = reorder_steps(&state, &id, order).await
    {
        return msg;
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Json(json!({ "ok": true })).into_response()
}

/// Rewrite the chain's step order in one transaction: every named task must
/// already be a step of THIS chain, and the chain's updated_at moves with
/// the change.
async fn reorder_steps(
    state: &AppState,
    chain_id: &str,
    order: &[(String, i32)],
) -> Result<(), Response> {
    let mut tx = match state.pg.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[workchains] reorder begin failed: {e}");
            return Err(thrown_internal_error());
        }
    };
    for (task_id, position) in order {
        let updated = sqlx::query(
            "update task_workchain_steps set position = $1 \
             where workchain_id = $2::uuid and task_id = $3::uuid",
        )
        .bind(position)
        .bind(chain_id)
        .bind(task_id)
        .execute(&mut *tx)
        .await;
        match updated {
            // rows_affected 0 = the order names work that is not in this
            // chain: writing the rest would silently drop it, so the whole
            // reorder refuses.
            Ok(res) if res.rows_affected() == 0 => {
                let _ = tx.rollback().await;
                return Err(house_error(
                    StatusCode::BAD_REQUEST,
                    "every position must name a step of this workchain",
                ));
            }
            Ok(_) => {}
            Err(e) => {
                let _ = tx.rollback().await;
                tracing::error!("[workchains] reorder write failed: {e}");
                return Err(thrown_internal_error());
            }
        }
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(chain_id)
        .execute(&mut *tx)
        .await
    {
        let _ = tx.rollback().await;
        tracing::error!("[workchains] reorder bump failed: {e}");
        return Err(thrown_internal_error());
    }
    if let Err(e) = tx.commit().await {
        tracing::error!("[workchains] reorder commit failed: {e}");
        return Err(thrown_internal_error());
    }
    Ok(())
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
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE", &id) {
        return gate;
    }
    let board_id = match write_gate(&state, &user.id, &id, "DELETE").await {
        Ok(b) => b,
        Err(gate) => return gate,
    };
    // The tickets are NOT touched: the cascade fires the step rows (their
    // workchain_id references), never the tasks.
    if let Err(e) = sqlx::query("delete from task_workchains where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[workchains] delete failed: {e}");
        return thrown_internal_error();
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Json(json!({ "ok": true })).into_response()
}

pub async fn post_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("workchains", "POST step", &id) {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let task_id = match uuid_member(obj, "taskId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let after = match optional_uuid_member(obj, "after") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let board_id = match write_gate(&state, &user.id, &id, "POST step").await {
        Ok(b) => b,
        Err(gate) => return gate,
    };
    // The task must exist, and on this chain's board — the same sentence
    // the dependency door answers a cross-board target with.
    let task: Option<(Option<String>,)> =
        match sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(&task_id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[workchains] task read on POST step failed: {e}");
                return thrown_internal_error();
            }
        };
    let Some((Some(task_board),)) = task else {
        return house_error(StatusCode::BAD_REQUEST, "no such ticket");
    };
    if task_board != board_id {
        return house_error(StatusCode::BAD_REQUEST, "must be a ticket on this board");
    }
    // v1: one chain per task. The precheck answers the friendly 409; the
    // unique index on task_id is what makes it true under a race.
    let in_chain: Option<(String,)> = match sqlx::query_as(
        "select s.workchain_id::text from task_workchain_steps s where s.task_id = $1::uuid",
    )
    .bind(&task_id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[workchains] chain read on POST step failed: {e}");
            return thrown_internal_error();
        }
    };
    if in_chain.is_some() {
        return house_error(
            StatusCode::CONFLICT,
            "this ticket is already in a workchain",
        );
    }
    // Insert: appended at the tail, or wedged after the named step with the
    // rest shifted down. `after` must name a step of THIS chain — the same
    // every-position rule the reorder enforces.
    let insert = match after {
        None => {
            sqlx::query(
                "insert into task_workchain_steps (workchain_id, task_id, position) \
                 select $1::uuid, $2::uuid, coalesce(max(position) + 1, 0) \
                 from task_workchain_steps where workchain_id = $1::uuid",
            )
            .bind(&id)
            .bind(&task_id)
            .execute(&state.pg)
            .await
        }
        Some(after_id) => {
            let shifted = sqlx::query(
                "update task_workchain_steps set position = position + 1 \
                 where workchain_id = $1::uuid and position > \
                       (select position from task_workchain_steps \
                        where workchain_id = $1::uuid and task_id = $2::uuid)",
            )
            .bind(&id)
            .bind(&after_id)
            .execute(&state.pg)
            .await;
            match shifted {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("[workchains] step shift failed: {e}");
                    return thrown_internal_error();
                }
            }
            sqlx::query(
                "insert into task_workchain_steps (workchain_id, task_id, position) \
                 select $1::uuid, $2::uuid, \
                        (select position + 1 from task_workchain_steps \
                         where workchain_id = $1::uuid and task_id = $3::uuid)",
            )
            .bind(&id)
            .bind(&task_id)
            .bind(&after_id)
            .execute(&state.pg)
            .await
        }
    };
    match insert {
        // rows_affected 0 = the `after` step is not in this chain (or the
        // position subselect found nothing): nothing was written.
        Ok(res) if res.rows_affected() == 0 => {
            return house_error(
                StatusCode::BAD_REQUEST,
                "'after' must name a step of this workchain",
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("[workchains] step insert failed: {e}");
            return thrown_internal_error();
        }
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[workchains] step bump failed: {e}");
        return thrown_internal_error();
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, task_id)): Path<(String, String)>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE step", &id) {
        return gate;
    }
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE step task", &task_id) {
        return gate;
    }
    let board_id = match write_gate(&state, &user.id, &id, "DELETE step").await {
        Ok(b) => b,
        Err(gate) => return gate,
    };
    // A miss is a quiet ok — the outcome the caller wanted (labels' delete
    // rule). The task is never touched: removing a step unlinks.
    if let Err(e) = sqlx::query(
        "delete from task_workchain_steps where workchain_id = $1::uuid and task_id = $2::uuid",
    )
    .bind(&id)
    .bind(&task_id)
    .execute(&state.pg)
    .await
    {
        tracing::error!("[workchains] step delete failed: {e}");
        return thrown_internal_error();
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[workchains] step delete bump failed: {e}");
        return thrown_internal_error();
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Json(json!({ "ok": true })).into_response()
}

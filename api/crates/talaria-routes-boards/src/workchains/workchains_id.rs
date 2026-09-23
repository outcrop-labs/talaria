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
use std::collections::{HashMap, HashSet};
use talaria_boards::{board_role, can_edit};
use talaria_body::{
    NumKind, array_msg, array_too_big_msg, number_member, object_msg, optional_boolean_member,
    optional_string_member, optional_uuid_member, parse, uuid_member, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_realtime_watch::{BoardEvent, RealtimeDeps, publish_board};
use talaria_session::require_user;
use talaria_state::AppState;

async fn chain_board(state: &AppState, id: &str, action: &str) -> Result<Option<String>, Response> {
    let board: Option<(Option<String>,)> =
        match sqlx::query_as("select board_id::text from task_workchains where id = $1::uuid")
            .bind(id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return Err(internal(
                    &format!("[workchains] board read on {action} failed"),
                    e,
                ));
            }
        };
    Ok(board.and_then(|(b,)| b))
}

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
        Err(e) => Err(internal(
            &format!("[workchains] role read on {action} failed"),
            e,
        )),
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
///
/// LINEAR SEMANTICS, GRAPH STORAGE (TALA-35): the wire shape is unchanged —
/// the api rewrites the chain's EDGES to a single line through the sent
/// order. A v1 client keeps working; the canvas reads edges, not positions.
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

/// nodes: [{ taskId, x, y }] — the canvas placement for the named steps
/// (TALA-35). Coordinates are free integers (the canvas is unbounded, axes
/// may go negative — a pan/zoom canvas keeps room left of the origin); every
/// named task must already be a step of the chain. A node may carry only one
/// axis — `{ x }` moves horizontally, y stays.
fn validate_nodes(
    obj: &Map<String, Value>,
) -> Result<Option<Vec<(String, Option<i32>, Option<i32>)>>, String> {
    let Some(v) = obj.get("nodes") else {
        return Ok(None);
    };
    let Some(entries) = v.as_array() else {
        return Err(array_msg(zod_type_name(v)));
    };
    if entries.len() > 200 {
        return Err(array_too_big_msg(200));
    }
    let mut out: Vec<(String, Option<i32>, Option<i32>)> = Vec::with_capacity(entries.len());
    for e in entries {
        let Some(o) = e.as_object() else {
            return Err(object_msg(zod_type_name(e)));
        };
        let task_id = uuid_member(o, "taskId")?;
        let x = match o.get("x") {
            None => None,
            Some(_) => Some(number_member(o, "x", NumKind::Int, -1e9, 1e9)? as i32),
        };
        let y = match o.get("y") {
            None => None,
            Some(_) => Some(number_member(o, "y", NumKind::Int, -1e9, 1e9)? as i32),
        };
        if x.is_none() && y.is_none() {
            return Err("every node carries an x, a y, or both".to_string());
        }
        out.push((task_id, x, y));
    }
    Ok(Some(out))
}

/// The linear order → edge rewrite. The sent order becomes ONE line: step i
/// wires to step i+1, every other edge drops. In one transaction: delete the
/// chain's edges, then insert the line. Positions update too (they remain
/// the read-order tiebreak the list walk uses).
async fn reorder_steps(
    state: &AppState,
    chain_id: &str,
    order: &[(String, i32)],
) -> Result<(), Response> {
    // The caller's entries sorted by their position — the LINE the caller
    // means, independent of the array's own order.
    let mut sorted: Vec<&(String, i32)> = order.iter().collect();
    sorted.sort_by_key(|(_, pos)| *pos);
    let mut tx = match state.pg.begin().await {
        Ok(t) => t,
        Err(e) => return Err(internal("[workchains] reorder begin failed", e)),
    };
    for (i, (task_id, position)) in sorted.iter().enumerate() {
        let updated = sqlx::query(
            "update task_workchain_steps set position = $1 \
             where workchain_id = $2::uuid and task_id = $3::uuid",
        )
        .bind(*position)
        .bind(chain_id)
        .bind((*task_id).clone())
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
                return Err(internal("[workchains] reorder write failed", e));
            }
        }
        // The line's next edge: step i → step i+1 (the last entry wires
        // nothing). Steps carry their own task_id; the edges reference the
        // STEP rows, resolved per task.
        if let Some((next_task, _)) = sorted.get(i + 1) {
            let inserted = sqlx::query(
                "insert into task_workchain_edges (workchain_id, from_step, to_step) \
                 select $1::uuid, f.id, t.id \
                 from task_workchain_steps f, task_workchain_steps t \
                 where f.workchain_id = $1::uuid and f.task_id = $2::uuid \
                   and t.workchain_id = $1::uuid and t.task_id = $3::uuid \
                 on conflict (workchain_id, from_step, to_step) do nothing",
            )
            .bind(chain_id)
            .bind(task_id)
            .bind(next_task)
            .execute(&mut *tx)
            .await;
            if let Err(e) = inserted {
                let _ = tx.rollback().await;
                return Err(internal("[workchains] edge insert failed", e));
            }
        }
    }
    // Drop every edge NOT in the new line (the graph collapses to it).
    if let Err(e) = sqlx::query(
        "delete from task_workchain_edges e \
         where e.workchain_id = $1::uuid \
           and not exists ( \
               select 1 from task_workchain_steps f, task_workchain_steps t \
               where f.workchain_id = e.workchain_id \
                 and t.workchain_id = e.workchain_id \
                 and (f.id = e.from_step and t.id = e.to_step) \
                 and (f.task_id, t.task_id) in ( \
                     select u.a, u.b from unnest($2::uuid[], $3::uuid[]) as u(a, b) \
                 ) \
           )",
    )
    .bind(chain_id)
    .bind(line_froms(&sorted))
    .bind(line_tos(&sorted))
    .execute(&mut *tx)
    .await
    {
        let _ = tx.rollback().await;
        return Err(internal("[workchains] edge prune failed", e));
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(chain_id)
        .execute(&mut *tx)
        .await
    {
        let _ = tx.rollback().await;
        return Err(internal("[workchains] reorder bump failed", e));
    }
    if let Err(e) = tx.commit().await {
        return Err(internal("[workchains] reorder commit failed", e));
    }
    Ok(())
}

/// The line's from- and to-task columns for the prune's unnest pair —
/// index-aligned: froms[i] → tos[i] is one kept edge.
fn line_froms(sorted: &[&(String, i32)]) -> Vec<String> {
    (0..sorted.len().saturating_sub(1))
        .map(|i| sorted[i].0.clone())
        .collect()
}
fn line_tos(sorted: &[&(String, i32)]) -> Vec<String> {
    (1..sorted.len()).map(|i| sorted[i].0.clone()).collect()
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("workchains", "PATCH", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match optional_string_member(obj, "name", 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let paused = match optional_boolean_member(obj, "paused") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let positions = match validate_positions(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let nodes = match validate_nodes(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let board_id = match write_gate(&state, &user.id, &id, "PATCH").await {
        Ok(b) => b,
        Err(gate) => return Err(gate),
    };
    // Nothing was asked for: ok, no write, no bump — the empty patch is the
    // workflows family's own quiet shape.
    if name.is_none() && paused.is_none() && positions.is_none() && nodes.is_none() {
        return Ok(Json(json!({ "ok": true })).into_response());
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
        return Ok(internal("[workchains] rename failed", e));
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
        return Ok(internal("[workchains] pause write failed", e));
    }
    if let Some(order) = &positions
        && let Err(msg) = reorder_steps(&state, &id, order).await
    {
        return Ok(msg);
    }
    if let Some(nodes) = &nodes
        && let Err(msg) = move_nodes(&state, &id, nodes).await
    {
        return Ok(msg);
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

/// The canvas placement write: one UPDATE per moved node, per axis. A miss
/// is a quiet no-op for that node (drag races a delete) — the canvas next
/// read shows the truth, and a stale drag never 500s a move.
async fn move_nodes(
    state: &AppState,
    chain_id: &str,
    nodes: &[(String, Option<i32>, Option<i32>)],
) -> Result<(), Response> {
    let mut tx = match state.pg.begin().await {
        Ok(t) => t,
        Err(e) => return Err(internal("[workchains] node move begin failed", e)),
    };
    for (task_id, x, y) in nodes {
        let res = sqlx::query(
            "update task_workchain_steps \
             set canvas_x = coalesce($1, canvas_x), canvas_y = coalesce($2, canvas_y) \
             where workchain_id = $3::uuid and task_id = $4::uuid",
        )
        .bind(*x)
        .bind(*y)
        .bind(chain_id)
        .bind(task_id)
        .execute(&mut *tx)
        .await;
        if let Err(e) = res {
            let _ = tx.rollback().await;
            return Err(internal("[workchains] node move write failed", e));
        }
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(chain_id)
        .execute(&mut *tx)
        .await
    {
        let _ = tx.rollback().await;
        return Err(internal("[workchains] node move bump failed", e));
    }
    if let Err(e) = tx.commit().await {
        return Err(internal("[workchains] node move commit failed", e));
    }
    Ok(())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE", &id) {
        return Ok(gate);
    }
    let board_id = match write_gate(&state, &user.id, &id, "DELETE").await {
        Ok(b) => b,
        Err(gate) => return Err(gate),
    };
    // The tickets are NOT touched: the cascade fires the step rows (their
    // workchain_id references), never the tasks.
    if let Err(e) = sqlx::query("delete from task_workchains where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        return Ok(internal("[workchains] delete failed", e));
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn post_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("workchains", "POST step", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let task_id = match uuid_member(obj, "taskId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let after = match optional_uuid_member(obj, "after") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let board_id = match write_gate(&state, &user.id, &id, "POST step").await {
        Ok(b) => b,
        Err(gate) => return Err(gate),
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
            Err(e) => return Ok(internal("[workchains] task read on POST step failed", e)),
        };
    let Some((Some(task_board),)) = task else {
        return Ok(house_error(StatusCode::BAD_REQUEST, "no such ticket"));
    };
    if task_board != board_id {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "must be a ticket on this board",
        ));
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
        Err(e) => return Ok(internal("[workchains] chain read on POST step failed", e)),
    };
    if in_chain.is_some() {
        return Ok(house_error(
            StatusCode::CONFLICT,
            "this ticket is already in a workchain",
        ));
    }
    // Insert: appended at the tail, or wedged after the named step with the
    // rest shifted down. `after` must name a step of THIS chain — the same
    // every-position rule the reorder enforces.
    // One transaction: the step lands AND joins the graph in the same commit.
    // The model is edges (TALA-35) — a step written without its wire derives
    // head (no predecessors), servable past every gate the chain exists to
    // apply, so the wiring is part of the write, not a follow-up.
    let mut tx = match state.pg.begin().await {
        Ok(t) => t,
        Err(e) => return Err(internal("[workchains] step add begin failed", e)),
    };
    // The reads the wiring needs, before the insert: an append wires from the
    // tail (the chain's highest position); a wedge splices the anchor's
    // downstream through the new step. Edges do not change with the insert,
    // so both read the pre-insert graph.
    let tail: Option<(String,)> = sqlx::query_as(
        "select task_id::text from task_workchain_steps \
         where workchain_id = $1::uuid \
         order by position desc, created_at desc limit 1",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| internal("[workchains] tail read on POST step failed", e))?;
    let after_succs: Vec<(String,)> = match &after {
        None => vec![],
        Some(after_id) => sqlx::query_as(
            "select ts.task_id::text \
             from task_workchain_edges e \
             join task_workchain_steps fs on fs.id = e.from_step \
             join task_workchain_steps ts on ts.id = e.to_step \
             where e.workchain_id = $1::uuid and fs.task_id = $2::uuid",
        )
        .bind(&id)
        .bind(after_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| internal("[workchains] anchor successors read on POST step failed", e))?,
    };
    let after_ref = after.as_deref();
    let insert = match after_ref {
        None => {
            sqlx::query(
                "insert into task_workchain_steps (workchain_id, task_id, position) \
                 select $1::uuid, $2::uuid, coalesce(max(position) + 1, 0) \
                 from task_workchain_steps where workchain_id = $1::uuid",
            )
            .bind(&id)
            .bind(&task_id)
            .execute(&mut *tx)
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
            .bind(after_id)
            .execute(&state.pg)
            .await;
            match shifted {
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Ok(internal("[workchains] step shift failed", e));
                }
            }
            sqlx::query(
                // The `where exists` is the 400 the rows_affected check below
                // expects: without it, a missing anchor puts a NULL-position
                // row in the select and the insert dies on the NOT NULL
                // constraint as a 500 instead of affecting zero rows.
                "insert into task_workchain_steps (workchain_id, task_id, position) \
                 select $1::uuid, $2::uuid, \
                        (select position + 1 from task_workchain_steps \
                         where workchain_id = $1::uuid and task_id = $3::uuid) \
                 where exists (select 1 from task_workchain_steps \
                               where workchain_id = $1::uuid and task_id = $3::uuid)",
            )
            .bind(&id)
            .bind(&task_id)
            .bind(after_id)
            .execute(&mut *tx)
            .await
        }
    };
    match insert {
        // rows_affected 0 = the `after` step is not in this chain (or the
        // position subselect found nothing): nothing was written. The
        // transaction rolls back — the wiring reads were only reads.
        Ok(res) if res.rows_affected() == 0 => {
            let _ = tx.rollback().await;
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "'after' must name a step of this workchain",
            ));
        }
        Ok(_) => {}
        Err(e) => {
            let _ = tx.rollback().await;
            return Ok(internal("[workchains] step insert failed", e));
        }
    }
    // ── The wiring (TALA-35): edges are the model, so the write maintains
    // them. An append wires from the tail; a wedge splices — the anchor's
    // downstream passes THROUGH the new step (every anchor → successor edge
    // rewires to new → succ, the v1 "wedged into the line" on a graph), and
    // after → new completes the splice. A wedge into a fan-out moves the
    // fan-out to the new step; the canvas reads edges, so it renders the
    // truth after the read refreshes.
    if after.is_none() {
        // Append: wire tail → new. The empty-chain case wires nothing (the
        // first step is a head with no predecessors).
        if let Some((tail_id,)) = tail {
            let wired = sqlx::query(
                "insert into task_workchain_edges (workchain_id, from_step, to_step) \
                 select $1::uuid, f.id, t.id \
                 from task_workchain_steps f, task_workchain_steps t \
                 where f.workchain_id = $1::uuid and f.task_id = $2::uuid \
                   and t.workchain_id = $1::uuid and t.task_id = $3::uuid \
                 on conflict (workchain_id, from_step, to_step) do nothing",
            )
            .bind(&id)
            .bind(&tail_id)
            .bind(&task_id)
            .execute(&mut *tx)
            .await;
            if let Err(e) = wired {
                let _ = tx.rollback().await;
                return Ok(internal("[workchains] append wiring failed", e));
            }
        }
    } else {
        let after_id = after.as_deref().expect("after checked above");
        for (succ_id,) in &after_succs {
            let rewired = sqlx::query(
                "update task_workchain_edges set to_step = ( \
                     select id from task_workchain_steps \
                     where workchain_id = $1::uuid and task_id = $2::uuid) \
                 where workchain_id = $1::uuid \
                   and from_step = ( \
                       select id from task_workchain_steps \
                       where workchain_id = $1::uuid and task_id = $3::uuid) \
                   and to_step = ( \
                       select id from task_workchain_steps \
                       where workchain_id = $1::uuid and task_id = $4::uuid)",
            )
            .bind(&id)
            .bind(&task_id)
            .bind(after_id)
            .bind(succ_id)
            .execute(&mut *tx)
            .await;
            match rewired {
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Ok(internal("[workchains] splice wiring failed", e));
                }
            }
        }
        // The new step's inbound edge: after → new.
        let wired = sqlx::query(
            "insert into task_workchain_edges (workchain_id, from_step, to_step) \
             select $1::uuid, f.id, t.id \
             from task_workchain_steps f, task_workchain_steps t \
             where f.workchain_id = $1::uuid and f.task_id = $2::uuid \
               and t.workchain_id = $1::uuid and t.task_id = $3::uuid \
             on conflict (workchain_id, from_step, to_step) do nothing",
        )
        .bind(&id)
        .bind(after_id)
        .bind(&task_id)
        .execute(&mut *tx)
        .await;
        if let Err(e) = wired {
            let _ = tx.rollback().await;
            return Ok(internal("[workchains] splice inbound wiring failed", e));
        }
    }
    if let Err(e) = tx.commit().await {
        return Err(internal("[workchains] step add commit failed", e));
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        return Ok(internal("[workchains] step bump failed", e));
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, task_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE step", &id) {
        return Ok(gate);
    }
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE step task", &task_id) {
        return Ok(gate);
    }
    let board_id = match write_gate(&state, &user.id, &id, "DELETE step").await {
        Ok(b) => b,
        Err(gate) => return Err(gate),
    };
    // One transaction: the neighbors are read, the line is spliced, the step
    // goes — the graph never shows a hole. A miss is a quiet ok — the
    // outcome the caller wanted (labels' delete rule). The task is never
    // touched: removing a step unlinks it.
    let mut tx = match state.pg.begin().await {
        Ok(t) => t,
        Err(e) => return Err(internal("[workchains] step delete begin failed", e)),
    };
    // The removed step's predecessors and successors, by task id. The splice
    // below rewires every pred → succ pair, so a straight line heals into a
    // line and a removed join hands its parents' work straight to its
    // children — the same "wedged out of the line" the v1 order edit did.
    let preds: Vec<(String,)> = sqlx::query_as(
        "select fs.task_id::text \
         from task_workchain_edges e \
         join task_workchain_steps fs on fs.id = e.from_step \
         join task_workchain_steps ts on ts.id = e.to_step \
         where e.workchain_id = $1::uuid and ts.task_id = $2::uuid",
    )
    .bind(&id)
    .bind(&task_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| internal("[workchains] step preds read on DELETE step failed", e))?;
    let succs: Vec<(String,)> = sqlx::query_as(
        "select ts.task_id::text \
         from task_workchain_edges e \
         join task_workchain_steps fs on fs.id = e.from_step \
         join task_workchain_steps ts on ts.id = e.to_step \
         where e.workchain_id = $1::uuid and fs.task_id = $2::uuid",
    )
    .bind(&id)
    .bind(&task_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| internal("[workchains] step succs read on DELETE step failed", e))?;
    // Insert-first: the new edges name steps that stay, and
    // on-conflict-do-nothing absorbs the pairs some other path already
    // carries (a diamond's shortcut) — an UPDATE would die on the unique
    // index there. Then the step goes and its own edges cascade with it.
    for (pred_id,) in &preds {
        for (succ_id,) in &succs {
            let wired = sqlx::query(
                "insert into task_workchain_edges (workchain_id, from_step, to_step) \
                 select $1::uuid, f.id, t.id \
                 from task_workchain_steps f, task_workchain_steps t \
                 where f.workchain_id = $1::uuid and f.task_id = $2::uuid \
                   and t.workchain_id = $1::uuid and t.task_id = $3::uuid \
                 on conflict (workchain_id, from_step, to_step) do nothing",
            )
            .bind(&id)
            .bind(pred_id)
            .bind(succ_id)
            .execute(&mut *tx)
            .await;
            if let Err(e) = wired {
                let _ = tx.rollback().await;
                return Ok(internal("[workchains] step delete splice failed", e));
            }
        }
    }
    let removed = sqlx::query(
        "delete from task_workchain_steps where workchain_id = $1::uuid and task_id = $2::uuid",
    )
    .bind(&id)
    .bind(&task_id)
    .execute(&mut *tx)
    .await;
    if let Err(e) = removed {
        let _ = tx.rollback().await;
        return Ok(internal("[workchains] step delete failed", e));
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(&id)
        .execute(&mut *tx)
        .await
    {
        let _ = tx.rollback().await;
        return Ok(internal("[workchains] step delete bump failed", e));
    }
    if let Err(e) = tx.commit().await {
        return Err(internal("[workchains] step delete commit failed", e));
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

// ── Wires (TALA-35) ──────────────────────────────────────────────────────────
// POST /api/workchains/{id}/edges { fromTaskId, toTaskId } draws one wire;
// DELETE /api/workchains/{id}/edges/{fromTaskId}/{toTaskId} removes it. A
// wire is an EDGE between two STEPS of this chain — both endpoints must
// already be steps (the canvas wires existing cards; joining an unchained
// ticket is the step-add surface's job), and the graph must stay acyclic:
// a wire that would let a step reach itself through its own successors is
// rejected with a 400 BEFORE any write (walk the successor graph from the
// target; finding the source there means from → to closes a cycle).

/// Cycle check: can `to_task` already REACH `from_task` through the chain's
/// successor edges? If yes, adding from → to closes a cycle. Breadth-first
/// over the adjacency list; a chain is dozens of nodes, so the O(V+E) walk
/// is cheaper than cleverness.
async fn would_cycle(
    pg: &sqlx::PgPool,
    chain_id: &str,
    from_task: &str,
    to_task: &str,
) -> Result<bool, sqlx::Error> {
    let edges: Vec<(String, String)> = sqlx::query_as(
        "select fs.task_id::text, ts.task_id::text \
         from task_workchain_edges e \
         join task_workchain_steps fs on fs.id = e.from_step \
         join task_workchain_steps ts on ts.id = e.to_step \
         where e.workchain_id = $1::uuid",
    )
    .bind(chain_id)
    .fetch_all(pg)
    .await?;
    let mut succ: HashMap<&str, Vec<&str>> = HashMap::new();
    for (f, t) in &edges {
        succ.entry(f.as_str()).or_default().push(t.as_str());
    }
    // Walk from `to_task`; a hit on `from_task` means the new wire back.
    let mut queue: Vec<&str> = vec![to_task];
    let mut seen: HashSet<&str> = HashSet::from([to_task]);
    while let Some(node) = queue.pop() {
        if node == from_task {
            return Ok(true);
        }
        for next in succ.get(node).into_iter().flatten() {
            if seen.insert(next) {
                queue.push(next);
            }
        }
    }
    Ok(false)
}

/// Both endpoints are steps of this chain — the same must-be-a-step rule
/// every step verb enforces. Returns the steps' uuids (the edges reference
/// step rows), or the 400's message.
async fn resolve_endpoints(
    pg: &sqlx::PgPool,
    chain_id: &str,
    from_task: &str,
    to_task: &str,
) -> Result<(String, String), String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select task_id::text, id::text from task_workchain_steps \
         where workchain_id = $1::uuid and task_id = any($2::uuid[])",
    )
    .bind(chain_id)
    .bind(vec![from_task.to_string(), to_task.to_string()])
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let find = |t: &str| {
        rows.iter()
            .find(|(task, _)| task == t)
            .map(|(_, id)| id.clone())
    };
    match (find(from_task), find(to_task)) {
        (Some(f), Some(t)) => Ok((f, t)),
        _ => Err("both endpoints must be steps of this workchain".to_string()),
    }
}

pub async fn post_edge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("workchains", "POST edge", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let from_task = match uuid_member(obj, "fromTaskId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let to_task = match uuid_member(obj, "toTaskId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if from_task == to_task {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "a wire needs two different tickets",
        ));
    }
    let board_id = match write_gate(&state, &user.id, &id, "POST edge").await {
        Ok(b) => b,
        Err(gate) => return Err(gate),
    };
    let (from_step, to_step) = match resolve_endpoints(&state.pg, &id, &from_task, &to_task).await {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match would_cycle(&state.pg, &id, &from_task, &to_task).await {
        Ok(false) => {}
        Ok(true) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "this wire would make the chain circular",
            ));
        }
        Err(e) => return Ok(internal("[workchains] cycle check failed", e)),
    }
    let inserted = sqlx::query(
        "insert into task_workchain_edges (workchain_id, from_step, to_step) \
         values ($1::uuid, $2::uuid, $3::uuid) \
         on conflict (workchain_id, from_step, to_step) do nothing",
    )
    .bind(&id)
    .bind(&from_step)
    .bind(&to_step)
    .execute(&state.pg)
    .await;
    match inserted {
        // A duplicate wire is a quiet ok — the outcome the caller wanted
        // (the delete verb's own rule, mirrored).
        Ok(_) => {}
        Err(e) => return Ok(internal("[workchains] edge insert failed", e)),
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        return Ok(internal("[workchains] edge bump failed", e));
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete_edge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, from_task, to_task)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE edge", &id) {
        return Ok(gate);
    }
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE edge from", &from_task) {
        return Ok(gate);
    }
    if let Some(gate) = talaria_params::uuid_gate("workchains", "DELETE edge to", &to_task) {
        return Ok(gate);
    }
    let board_id = match write_gate(&state, &user.id, &id, "DELETE edge").await {
        Ok(b) => b,
        Err(gate) => return Err(gate),
    };
    // A miss is a quiet ok — the outcome the caller wanted (the step
    // delete's own rule). The tasks are never touched: removing a wire
    // unlinks.
    if let Err(e) = sqlx::query(
        "delete from task_workchain_edges e \
         using task_workchain_steps fs, task_workchain_steps ts \
         where e.workchain_id = $1::uuid \
           and fs.id = e.from_step and ts.id = e.to_step \
           and fs.task_id = $2::uuid and ts.task_id = $3::uuid",
    )
    .bind(&id)
    .bind(&from_task)
    .bind(&to_task)
    .execute(&state.pg)
    .await
    {
        return Ok(internal("[workchains] edge delete failed", e));
    }
    if let Err(e) = sqlx::query("update task_workchains set updated_at = now() where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        return Ok(internal("[workchains] edge delete bump failed", e));
    }
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    bump(&realtime, &board_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

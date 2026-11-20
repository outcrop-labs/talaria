// /api/boards/{id}/sync — the board's ClickUp sync (talaria-board-sync,
// docs/BOARD-SYNC.md). One-way, ClickUp → Talaria: a board owner connects one
// ClickUp list with a personal token; a durable `clickup-sync` run pulls its
// tasks, comments and attachments into tickets on a schedule and on Sync now.
//   GET    → {sync, disconnected}: the projection (last/next sync, the live
//            run's phase, state, problems, warnings) — any member.
//   POST   connect {token, listId, statusMap, options?, intervalMinutes?} →
//            {sync, runId}; seals the token, validates the mapping, starts the
//            first sync — owner. 409 when the board is already connected.
//   PUT    edit {statusMap?, options?, intervalMinutes?, paused?} → {sync}; a
//            mapping change re-places what the old mapping placed; `paused`
//            true freezes the sync (no scheduled run, Sync now or re-sync
//            until it is resumed, and the live run is stopped), false
//            resumes it — owner.
//   DELETE disconnect → {ok}: future syncs stop; every imported ticket stays,
//            editable, owned by Talaria — owner.
// The token is never logged and never answered; the projection carries its
// last four characters and the ClickUp account it acts as.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_board_sync::mapping::StatusMap;
use talaria_board_sync::ops::{self, ConnectInput, OpError, UpdateInput};
use talaria_board_sync::store::SyncOptions;
use talaria_body::{optional_boolean_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// A refusal as the house envelope; a 500 is logged with its context.
pub(crate) fn op_error(e: OpError, context: &str) -> Response {
    if e.status >= 500 && e.status != 502 {
        return internal(context, &e.message);
    }
    house_error(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST),
        &e.message,
    )
}

/// `statusMap`: an object of ClickUp status name → board column key.
pub(crate) fn status_map_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<StatusMap>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let Some(m) = v.as_object() else {
        return Err(format!(
            "{key} must be an object of ClickUp status → column key"
        ));
    };
    if m.len() > 200 {
        return Err(format!("{key} has too many statuses (max 200)"));
    }
    let mut out = StatusMap::new();
    for (name, col) in m {
        let Some(col) = col.as_str().filter(|c| !c.is_empty() && c.len() <= 100) else {
            return Err(format!(
                "{key} maps \"{name}\" to something that is not a column key"
            ));
        };
        out.insert(name.clone(), col.to_string());
    }
    Ok(Some(out))
}

/// `options`: {includeClosed?, comments?, attachments?, notifyApps?} over a
/// base; anything else in the object is ignored.
pub(crate) fn options_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    base: SyncOptions,
) -> Result<Option<SyncOptions>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let Some(m) = v.as_object() else {
        return Err(format!("{key} must be an object"));
    };
    let mut o = base;
    for (k, slot) in [
        ("includeClosed", &mut o.include_closed),
        ("comments", &mut o.comments),
        ("attachments", &mut o.attachments),
        ("notifyApps", &mut o.notify_apps),
    ] {
        if let Some(b) = m.get(k) {
            *slot = b
                .as_bool()
                .ok_or_else(|| format!("{key}.{k} must be true or false"))?;
        }
    }
    Ok(Some(o))
}

/// `intervalMinutes`: absent → unchanged, null → manual only, a number → that
/// cadence (the engine checks it is one on offer).
pub(crate) fn interval_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Option<i32>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => v
            .as_i64()
            .filter(|n| (1..=100_000).contains(n))
            .map(|n| Some(Some(n as i32)))
            .ok_or_else(|| format!("{key} must be null (manual), 15, 60 or 1440")),
    }
}

async fn projection(state: &AppState, board_id: &str) -> Result<Value, Response> {
    talaria_board_sync::status::board_sync_status(&state.pg, board_id, talaria_agent_auth::now_ms())
        .await
        .map_err(|e| internal("[board-sync] status read failed", e))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET sync", &id) {
        return Ok(gate);
    }
    match talaria_boards::board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[board-sync] role read on GET sync failed", e)),
    }
    let body = projection(&state, &id).await?;
    Ok(Json(body).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST sync", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let token = match string_member(obj, "token", 8, 400) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let list_id = match string_member(obj, "listId", 1, 100) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let status_map = match status_map_member(obj, "statusMap") {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "statusMap is required — confirm the mapping preview",
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let options = match options_member(obj, "options", SyncOptions::default()) {
        Ok(o) => o.unwrap_or_default(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let interval = match interval_member(obj, "intervalMinutes") {
        Ok(v) => v.unwrap_or(talaria_board_sync::store::DEFAULT_INTERVAL),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let input = ConnectInput {
        token,
        list_id,
        status_map,
        options,
        interval_minutes: interval,
    };
    let started = match ops::connect(&state, &id, &user.id, input).await {
        Ok((_, started)) => started,
        Err(e) => return Ok(op_error(e, "[board-sync] connect failed")),
    };
    let body = projection(&state, &id).await?;
    Ok(Json(json!({ "sync": body["sync"], "runId": started.id() })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "PUT sync", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let status_map = match status_map_member(obj, "statusMap") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let current = match talaria_board_sync::store::live_sync(&state.pg, &id).await {
        Ok(s) => s.map(|s| s.options).unwrap_or_default(),
        Err(e) => return Ok(internal("[board-sync] sync read on PUT failed", e)),
    };
    let options = match options_member(obj, "options", current) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let interval = match interval_member(obj, "intervalMinutes") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // true freezes the sync until an owner resumes it; false resumes it.
    let paused = match optional_boolean_member(obj, "paused") {
        Ok(v) => v,
        Err(_) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "paused must be true or false",
            ));
        }
    };
    let input = UpdateInput {
        status_map,
        options,
        interval_minutes: interval,
        paused,
    };
    if let Err(e) = ops::update(&state, &id, &user.id, input).await {
        return Ok(op_error(e, "[board-sync] update failed"));
    }
    let body = projection(&state, &id).await?;
    Ok(Json(json!({ "sync": body["sync"] })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "DELETE sync", &id) {
        return Ok(gate);
    }
    if let Err(e) = ops::disconnect(&state, &id, &user.id).await {
        return Ok(op_error(e, "[board-sync] disconnect failed"));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

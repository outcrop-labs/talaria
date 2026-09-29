// Agent Google Sheets. Reads are free. A write ALWAYS queues for a human —
// there is no create-it-yourself exemption here as there is for Docs, because
// there is no "sheet this agent created" to exempt: the agent has no tool that
// makes a Google Sheet, only ones that change an existing person's.
//
// GET  /sheets/{id}?range=  read cells
// POST /sheets/{id}         queue a cell write

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::oauth::query_pairs;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_api_facades::google::sheets::read_sheet_with_token;
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

const NOT_CONNECTED: &str =
    "No Google account is connected for this agent (its owner, or the org account).";

fn not_connected() -> Response {
    house_error_msg(StatusCode::CONFLICT, "not_connected", NOT_CONNECTED)
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Sheets") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    let range = query_pairs(uri.query()).get("range").cloned();
    match read_sheet_with_token(&google.token, &id, range.as_deref()).await {
        Ok(sheet) => Ok(Json(sheet).into_response()),
        Err(e) => Ok(google_fail(e, "Sheets")),
    }
}

/// The rows a write carries. A cell arrives as a string; anything else is
/// refused rather than coerced, because `USER_ENTERED` will interpret what it
/// is given and a silently stringified number is a different cell than the
/// caller meant.
fn rows_from(body: &Value) -> Result<Vec<Vec<String>>, String> {
    let Some(rows) = body.get("rows").and_then(Value::as_array) else {
        return Err("Invalid input: expected rows to be an array of arrays".into());
    };
    if rows.len() > 5_000 {
        return Err("Too many rows: 5000 at most in one write".into());
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(cells) = row.as_array() else {
            return Err("Invalid input: expected each row to be an array".into());
        };
        if cells.len() > 256 {
            return Err("Too many columns: 256 at most in one write".into());
        }
        let mut line = Vec::with_capacity(cells.len());
        for cell in cells {
            match cell {
                Value::String(s) => line.push(s.clone()),
                Value::Null => line.push(String::new()),
                _ => {
                    return Err(
                        "Invalid input: every cell must be a string (send 42 as \"42\")".into(),
                    );
                }
            }
        }
        out.push(line);
    }
    Ok(out)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Sheets") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let range = match string_member(obj, "range", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let rows = match rows_from(&parsed) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| {
                internal(
                    "[integrations/google/agent/sheets] principal read failed",
                    e,
                )
            })?
    {
        return Ok(denied);
    }
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/sheets] principal read failed",
                e,
            ));
        }
    };
    // One pending write per file AND range. Two edits to different ranges of
    // the same sheet are different intentions and both deserve a card; a
    // repeat of the same range is the agent retrying.
    let existing: Option<(String,)> = match sqlx::query_as(
        "select id::text from google_pending_actions \
         where kind = 'sheet_update' and status = 'pending' and agent_model = $1 \
           and payload->>'fileId' = $2 and payload->>'range' = $3 \
         order by created_at desc limit 1",
    )
    .bind(&agent_model)
    .bind(&id)
    .bind(&range)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/sheets] dedupe read failed",
                e,
            ));
        }
    };
    if let Some((pending_id,)) = existing {
        return Ok(Json(json!({
            "pending": { "id": pending_id, "status": "pending", "kind": "sheet_update" },
            "message": "A write to this range is already waiting for approval — nothing new queued.",
        }))
        .into_response());
    }
    let mut payload = serde_json::Map::new();
    payload.insert("fileId".into(), json!(id));
    payload.insert("range".into(), json!(range));
    payload.insert("rows".into(), json!(rows));
    if let Some(n) = &note {
        payload.insert("note".into(), json!(n));
    }
    let summary = format!("Write {} cell rows to {range}", rows.len());
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let queued = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind: "sheet_update",
            summary: &summary,
            payload: &Value::Object(payload),
            agent_model: &agent_model,
            owner_user_id: principal.owner_user_id.as_deref(),
            is_org: principal.is_org,
            principal_kind: principal.kind.as_str(),
        },
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/sheets] queue failed",
                e,
            ));
        }
    };
    Ok(super::integrations_google_agent_queue::answer_queued(
        &state.pg,
        queued,
        "A write to this range is already waiting for approval — nothing new queued.",
        "Queued — waiting for the owner to approve before the sheet changes.",
        "Queued — waiting for an admin to approve before the sheet changes.",
    )
    .await)
}

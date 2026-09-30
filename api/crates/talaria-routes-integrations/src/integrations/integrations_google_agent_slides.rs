// Agent Google Slides. Reading is free; replacing text ALWAYS queues for a
// human — there is no deck an agent owns, so there is no immediate path.
//
// There is no authoring here, on purpose: see the crate header in
// talaria-google-slides for why a layout belongs to whoever made it.
//
// GET  /slides/{id}   the deck's text, slide by slide
// POST /slides/{id}   queue a find-and-replace across the deck

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_api_facades::google::slides::read_deck_with_token;
use talaria_body::{optional_boolean_member, parse};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

const NOT_CONNECTED: &str =
    "No Google account is connected for this agent (its owner, or the org account).";

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Slides") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            NOT_CONNECTED,
        ));
    };
    match read_deck_with_token(&google.token, &id).await {
        Ok(deck) => Ok(Json(deck).into_response()),
        Err(e) => Ok(google_fail(e, "Slides")),
    }
}

/// The find/replace pairs a write carries, validated for shape. `find` must be
/// non-empty — an empty needle would match everywhere and rewrite the whole
/// deck — and `replace` may be empty, because deleting a phrase is a real edit.
fn replacements_from(body: &Value) -> Result<Vec<Value>, String> {
    const MAX_PAIRS: usize = 50;
    let Some(items) = body.get("replacements").and_then(Value::as_array) else {
        return Err("Invalid input: expected replacements to be an array".into());
    };
    if items.is_empty() {
        return Err("Invalid input: expected at least one replacement".into());
    }
    if items.len() > MAX_PAIRS {
        return Err(format!("Too many replacements: {MAX_PAIRS} at most"));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(pair) = item.as_object() else {
            return Err("Invalid input: expected each replacement to be an object".into());
        };
        let find = pair.get("find").and_then(Value::as_str).unwrap_or_default();
        if find.is_empty() || find.len() > 1_000 {
            return Err("Invalid input: expected find to be the text to replace".into());
        }
        let replace = pair
            .get("replace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if replace.len() > 1_000 {
            return Err("Invalid input: replace is too long".into());
        }
        out.push(json!({ "find": find, "replace": replace }));
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
    if let Some(denied) = refuse_legacy(&caller, "Google Slides") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let match_case = match optional_boolean_member(obj, "matchCase") {
        Ok(v) => v.unwrap_or(true),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let replacements = match replacements_from(&parsed) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| {
                internal(
                    "[integrations/google/agent/slides] principal read failed",
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
                "[integrations/google/agent/slides] principal read failed",
                e,
            ));
        }
    };
    // One pending change per deck. Unlike a sheet there is no range to
    // distinguish two intentions, and a second batch of replacements while the
    // first waits is the agent retrying rather than asking for something else.
    let existing: Option<(String,)> = match sqlx::query_as(
        "select id::text from google_pending_actions \
         where kind = 'slides_update' and status = 'pending' and agent_model = $1 \
           and payload->>'fileId' = $2 \
         order by created_at desc limit 1",
    )
    .bind(&agent_model)
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/slides] dedupe read failed",
                e,
            ));
        }
    };
    if let Some((pending_id,)) = existing {
        return Ok(Json(json!({
            "pending": { "id": pending_id, "status": "pending", "kind": "slides_update" },
            "message": "A change to this deck is already waiting for approval — nothing new queued.",
        }))
        .into_response());
    }
    let mut payload = serde_json::Map::new();
    payload.insert("fileId".into(), json!(id));
    payload.insert("replacements".into(), Value::Array(replacements.clone()));
    payload.insert("matchCase".into(), json!(match_case));
    let summary = format!("Replace {} phrase(s) in a Slides deck", replacements.len());
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let queued = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind: "slides_update",
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
                "[integrations/google/agent/slides] queue failed",
                e,
            ));
        }
    };
    Ok(super::integrations_google_agent_queue::answer_queued(
        &state.pg,
        queued,
        "A change to this deck is already waiting for approval — nothing new queued.",
        "Queued — waiting for the owner to approve before the deck changes.",
        "Queued — waiting for an admin to approve before the deck changes.",
    )
    .await)
}

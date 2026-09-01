// /api/integrations/google/agent/calendar — port of
// ui/src/routes/api/integrations/google.agent.calendar.ts. Agent-facing
// calendar: a personal assistant acts as its owner; a general fleet agent
// acts on the shared ORG calendar.
// GET  → read upcoming events (free)
// POST → DRAFT an event; queued for approval (the owner, or an admin for org).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::agent_auth::{AgentSubject, refuse_legacy, require_agent};
use crate::body::{
    as_object, optional_boolean_member, optional_email_array_member, optional_max_string_member,
    parse, string_member,
};
use crate::error::{house_error, house_error_msg, thrown_internal_error};
use crate::google_agent::{resolve_agent_google, resolve_agent_principal};
use crate::google_calendar::list_upcoming_events_with_token;
use crate::google_errors::{GoogleError, google_fail_with};
use crate::google_org::get_org_targets;
use crate::google_pending_actions::{QueueAction, queue_action};
use crate::realtime::RealtimeDeps;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // Acting as a HUMAN — the owner's calendar (or the shared org one). A
    // legacy shared-key caller only ASSERTS which agent it is, so it never
    // reaches a token; the refusal names the container to roll.
    if let Some(denied) = refuse_legacy(&caller, "Calendar access") {
        return denied;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "No Google account is connected for this agent (its owner, or the org account).",
        );
    };
    let calendar_id = if google.principal == "org" {
        match get_org_targets(&state.pg).await {
            Ok(t) => t.calendar_id,
            Err(e) => {
                tracing::error!("[integrations/google/agent] org targets read failed: {e}");
                return thrown_internal_error();
            }
        }
    } else {
        None
    };
    match list_upcoming_events_with_token(&google.token, now_ms(), 10, calendar_id.as_deref()).await
    {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(e) => google_fail_with(GoogleError::from(e), "Calendar", "calendar_error"),
    }
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    if let Some(denied) = refuse_legacy(&caller, "Calendar access") {
        return denied;
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let summary = match string_member(obj, "summary", 1, 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match optional_max_string_member(obj, "description", 8000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let location = match optional_max_string_member(obj, "location", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let start = match string_member(obj, "start", 4, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let end = match string_member(obj, "end", 4, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let all_day = match optional_boolean_member(obj, "allDay") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let attendees = match optional_email_array_member(obj, "attendees", 50) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[integrations/google/agent] principal read failed: {e}");
            return thrown_internal_error();
        }
    };
    // The payload IS the validated draft, stored exactly as drafted and
    // executed as stored at approve time — optional members ride only when
    // the request carried them (JSON.stringify drops undefined).
    let mut payload = serde_json::Map::new();
    payload.insert("summary".into(), json!(summary));
    if let Some(d) = &description {
        payload.insert("description".into(), json!(d));
    }
    if let Some(l) = &location {
        payload.insert("location".into(), json!(l));
    }
    payload.insert("start".into(), json!(start));
    payload.insert("end".into(), json!(end));
    if let Some(a) = all_day {
        payload.insert("allDay".into(), json!(a));
    }
    if let Some(list) = &attendees {
        payload.insert("attendees".into(), json!(list));
    }
    let draft_summary = format!("Event: {summary} ({start})");
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let action = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind: "calendar_create",
            summary: &draft_summary,
            payload: &Value::Object(payload),
            agent_model: &agent_model,
            owner_user_id: principal.owner_user_id.as_deref(),
            is_org: principal.is_org,
        },
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[integrations/google/agent] queue failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "pending": { "id": action.id, "status": "pending" },
        "message": if principal.is_org {
            "Drafted — waiting for an admin to approve."
        } else {
            "Drafted — waiting for the owner to approve."
        },
    }))
    .into_response()
}

/// Date.now() — the one clock the agent calendar surface reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

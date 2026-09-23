// /api/integrations/google/agent/calendar. Agent-facing calendar: a personal
// assistant acts as its owner; a general fleet agent acts on the shared ORG
// calendar.
// GET  → read upcoming events (free)
// POST → DRAFT an event; queued for approval (the owner, or an admin for org).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::calendar::list_upcoming_events_with_token;
use talaria_api_facades::google::errors::{GoogleError, google_fail_with};
use talaria_api_facades::google::org::get_org_targets;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_body::{
    optional_boolean_member, optional_email_array_member, optional_max_string_member, parse,
    string_member,
};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    // Acting as a HUMAN — the owner's calendar (or the shared org one). A
    // legacy shared-key caller only ASSERTS which agent it is, so it never
    // reaches a token; the refusal names the container to roll.
    if let Some(denied) = refuse_legacy(&caller, "Calendar access") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "No Google account is connected for this agent (its owner, or the org account).",
        ));
    };
    let calendar_id = if google.principal == "org" {
        match get_org_targets(&state.pg).await {
            Ok(t) => t.calendar_id,
            Err(e) => {
                return Ok(internal(
                    "[integrations/google/agent] org targets read failed",
                    e,
                ));
            }
        }
    } else {
        None
    };
    Ok(
        match list_upcoming_events_with_token(&google.token, now_ms(), 10, calendar_id.as_deref())
            .await
        {
            Ok(events) => Json(json!({ "events": events })).into_response(),
            Err(e) => google_fail_with(GoogleError::from(e), "Calendar", "calendar_error"),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return Ok(gate),
    };
    if let Some(denied) = refuse_legacy(&caller, "Calendar access") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let summary = match string_member(obj, "summary", 1, 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let description = match optional_max_string_member(obj, "description", 8000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let location = match optional_max_string_member(obj, "location", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let start = match string_member(obj, "start", 4, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let end = match string_member(obj, "end", 4, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let all_day = match optional_boolean_member(obj, "allDay") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let attendees = match optional_email_array_member(obj, "attendees", 50) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent] principal read failed",
                e,
            ));
        }
    };
    let talking_to = match super::integrations_google_agent_queue::streaming_conversation_user(
        &state.pg,
        &agent_model,
    )
    .await
    {
        Ok(user) => user,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent] conversation owner read failed",
                e,
            ));
        }
    };
    if let Some(reason) = super::integrations_google_agent_queue::conversation_owner_conflict(
        &principal,
        talking_to.as_deref(),
    ) {
        return Ok(house_error(StatusCode::CONFLICT, &reason));
    }

    // The payload IS the validated draft, stored exactly as drafted and
    // executed as stored at approve time — optional members ride only when
    // the request carried them.
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
    let queued = match queue_action(
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
        Ok(q) => q,
        Err(e) => return Ok(internal("[integrations/google/agent] queue failed", e)),
    };
    if !principal.is_org
        && let Ok(Some(conv)) = talaria_chips::live_conversation_id(&state.pg, &agent_model).await
        && talaria_chips::tool_unlocked(&state.pg, &conv, "draft_calendar_event")
            .await
            .unwrap_or(false)
        && let Some(owner) = principal.owner_user_id.as_deref()
    {
        let sb = state.secretbox().await.unwrap_or_default();
        let id = queued.action.id.clone();
        return Ok(
            match talaria_api_facades::google::pending_actions::decide_action(
                &state.pg,
                &sb,
                &id,
                owner,
                false,
                "approve",
                talaria_agent_auth::now_ms(),
            )
            .await
            {
                Ok(Some(outcome)) => super::integrations_google_agent_queue::answer_executed(
                    &id,
                    &outcome.status,
                    "Created — this conversation already unlocked draft_calendar_event.",
                    "Unlocked draft_calendar_event did not create the event",
                ),
                Ok(None) => house_error(
                    StatusCode::CONFLICT,
                    "unlocked event disappeared before it could be created — do not report it as created",
                ),
                Err(e) => internal("[integrations/google/agent] unlocked event failed", e),
            },
        );
    }
    // Calendar has no signature in the dedupe yet, so `already_pending` is
    // false from this route today — the wording branch exists so the kind
    // cannot join the dedupe without answering what its message says.
    Ok(super::integrations_google_agent_queue::answer_queued(
        &state.pg,
        queued,
        "An identical event is already waiting for approval — nothing new queued.",
        "Drafted — waiting for the owner to approve.",
        "Drafted — waiting for an admin to approve.",
    )
    .await)
}

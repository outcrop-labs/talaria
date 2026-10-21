// /api/integrations/google/agent/calendar. Agent-facing calendar: a personal
// assistant acts as its owner; a general fleet agent acts on the shared ORG
// calendar.
// GET  → read upcoming events (free)
// POST → DRAFT an event; queued for approval (the owner, or an admin for org).
//        A still-live grant from an earlier approval in the same conversation
//        covers ONE more event to the same attendees within 30 minutes.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::calendar::list_events_in_window_with_token;
use talaria_api_facades::google::errors::{GoogleError, google_fail_with};
use talaria_api_facades::google::oauth::query_pairs;
use talaria_api_facades::google::org::get_org_targets;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_body::{
    optional_boolean_member, optional_email_array_member, optional_max_string_member, parse,
    string_member,
};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
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
    let q = query_pairs(uri.query());
    let time_min = q
        .get("timeMin")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| talaria_agent_auth::epoch_ms_to_iso(now_ms()));
    let time_max = q.get("timeMax").filter(|s| !s.is_empty()).cloned();
    let max_results = q
        .get("maxResults")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 50);
    Ok(
        match list_events_in_window_with_token(
            &google.token,
            calendar_id.as_deref(),
            &time_min,
            time_max.as_deref(),
            max_results,
        )
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
    let start = match instant_member(obj, "start") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let end = match instant_member(obj, "end") {
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
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| {
                internal(
                    "[integrations/google/agent] principal connection read failed",
                    e,
                )
            })?
    {
        return Ok(denied);
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
            principal_kind: principal.kind.as_str(),
        },
    )
    .await
    {
        Ok(q) => q,
        Err(e) => return Ok(internal("[integrations/google/agent] queue failed", e)),
    };
    // A prior approval in this conversation left a BOUNDED grant for
    // draft_calendar_event — it expires, it covers one call, and only an
    // event whose attendees are among the ones the approved event named
    // (`talaria_chips::tool_unlocked` spends it, once, under a row lock).
    // The owner is resolved before the grant is spent.
    let recipients = talaria_chips::normalize_recipients(
        attendees
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(String::as_str),
    );
    if !principal.is_org
        && let Some(owner) = principal.owner_user_id.as_deref()
        && let Ok(Some(conv)) = talaria_chips::live_conversation_id(&state.pg, &agent_model).await
        && talaria_chips::tool_unlocked(&state.pg, &conv, "draft_calendar_event", &recipients)
            .await
            .unwrap_or(false)
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
                    "Created — the earlier approval covered this one; the next needs its own.",
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

async fn queue_calendar_change(
    state: &AppState,
    headers: &HeaderMap,
    body: Bytes,
    kind: &'static str,
    summary: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Calendar access") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| {
                internal(
                    "[integrations/google/agent] principal connection read failed",
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
                "[integrations/google/agent] principal read failed",
                e,
            ));
        }
    };
    let _ = body;
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let queued = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind,
            summary,
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
        Err(e) => return Ok(internal("[integrations/google/agent] queue failed", e)),
    };
    Ok(super::integrations_google_agent_queue::answer_queued(
        &state.pg,
        queued,
        "An identical calendar change is already waiting — nothing new queued.",
        "Queued — waiting for the owner to approve before the calendar changes.",
        "Queued — waiting for an admin to approve before the calendar changes.",
    )
    .await)
}

pub async fn post_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let event_id = match string_member(obj, "eventId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut payload = serde_json::Map::new();
    payload.insert("eventId".into(), json!(event_id));
    // The same bounds the draft route applies: a change the approver's card
    // shows is validated before it is stored, not when Google refuses it.
    match optional_max_string_member(obj, "summary", 500) {
        Ok(Some(v)) => {
            payload.insert("summary".into(), json!(v));
        }
        Ok(None) => {}
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
    for key in ["start", "end"] {
        match optional_instant_member(obj, key) {
            Ok(Some(v)) => {
                payload.insert(key.into(), json!(v));
            }
            Ok(None) => {}
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    }
    match optional_boolean_member(obj, "allDay") {
        Ok(Some(all_day)) => {
            payload.insert("allDay".into(), json!(all_day));
        }
        Ok(None) => {}
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
    let summary = format!("Update calendar event {event_id}");
    queue_calendar_change(&state, &headers, body, "calendar_update", &summary, payload).await
}

pub async fn post_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let event_id = match string_member(obj, "eventId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut payload = serde_json::Map::new();
    payload.insert("eventId".into(), json!(event_id));
    let summary = format!("Cancel calendar event {event_id}");
    queue_calendar_change(&state, &headers, body, "calendar_cancel", &summary, payload).await
}

/// Queue a meeting. The agenda markdown is stored only. Nothing is created
/// in Google until a human approves.
pub async fn post_meeting(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let summary_text = match string_member(obj, "summary", 1, 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let start = match instant_member(obj, "start") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let end = match instant_member(obj, "end") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The agenda becomes a Doc at approve time; the attendees are the same
    // validated, capped list the draft route takes — an invite is a send.
    let agenda = match optional_max_string_member(obj, "agenda", 50_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let attendees = match optional_email_array_member(obj, "attendees", 50) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut payload = serde_json::Map::new();
    payload.insert("summary".into(), json!(summary_text));
    payload.insert("start".into(), json!(start));
    payload.insert("end".into(), json!(end));
    payload.insert("meet".into(), json!(true));
    if let Some(agenda) = agenda {
        payload.insert("agenda".into(), json!(agenda));
    }
    if let Some(list) = attendees {
        payload.insert("attendees".into(), json!(list));
    }
    let summary = format!("Meeting: {summary_text} ({start})");
    queue_calendar_change(&state, &headers, body, "meeting_create", &summary, payload).await
}

/// A calendar instant as Google takes it: an all-day date (`2026-10-20`) or
/// an RFC 3339 timestamp (`2026-10-20T09:00:00-07:00`, `…Z`). Bounded,
/// ASCII, and shaped — Calendar rejects anything else at approve time,
/// which is after the approver already read the card.
fn calendar_instant_ok(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 10 || b.len() > 40 {
        return false;
    }
    let date_ok = b[..10].iter().enumerate().all(|(i, c)| {
        if i == 4 || i == 7 {
            *c == b'-'
        } else {
            c.is_ascii_digit()
        }
    });
    if !date_ok {
        return false;
    }
    if b.len() == 10 {
        return true;
    }
    b[10] == b'T'
        && b.len() >= 16
        && b[11..]
            .iter()
            .all(|c| c.is_ascii_digit() || matches!(c, b':' | b'.' | b'+' | b'-' | b'Z'))
}

fn instant_reason(key: &str) -> String {
    format!("{key} must be a date (YYYY-MM-DD) or an RFC 3339 timestamp")
}

fn instant_member(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let v = string_member(obj, key, 10, 40)?;
    if !calendar_instant_ok(&v) {
        return Err(instant_reason(key));
    }
    Ok(v)
}

fn optional_instant_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match optional_max_string_member(obj, key, 40)? {
        Some(v) if !calendar_instant_ok(&v) => Err(instant_reason(key)),
        v => Ok(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_calendar_instant_is_a_date_or_an_rfc3339_timestamp() {
        for ok in [
            "2026-10-20",
            "2026-10-20T09:00:00Z",
            "2026-10-20T09:00:00-07:00",
            "2026-10-20T09:00:00.000+02:00",
        ] {
            assert!(calendar_instant_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "tomorrow",
            "2026-10-2",
            "2026/10/20",
            "2026-10-20 09:00",
            "2026-10-20T09",
            "2026-10-20T09:00:00Z; drop table",
            "2026-10-20T09:00:00Z000000000000000000000000000",
        ] {
            assert!(!calendar_instant_ok(bad), "{bad}");
        }
    }

    #[test]
    fn meeting_and_update_bodies_take_the_draft_bounds() {
        let obj = json!({
            "start": "2026-10-20T09:00:00Z",
            "end": "sometime next week",
            "short": "soon",
        });
        let obj = obj.as_object().unwrap();
        assert_eq!(
            instant_member(obj, "start").unwrap(),
            "2026-10-20T09:00:00Z"
        );
        // The length rung answers first (zod's first issue), the shape rung
        // names the format.
        assert!(instant_member(obj, "short").is_err());
        assert!(instant_member(obj, "end").unwrap_err().contains("RFC 3339"));
        assert!(optional_instant_member(obj, "missing").unwrap().is_none());
        assert!(optional_instant_member(obj, "end").is_err());
    }
}

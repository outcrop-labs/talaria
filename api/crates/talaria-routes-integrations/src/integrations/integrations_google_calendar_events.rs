// /api/integrations/google/calendar/events.
// GET  → upcoming events on the user's primary calendar
// POST → create an event

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::now_ms;
use talaria_api_facades::google::calendar::{
    CalendarError, CreateEventInput, create_event, list_upcoming_events,
};
use talaria_api_facades::google::errors::{GoogleError, google_fail_with};
use talaria_body::{
    optional_boolean_member, optional_email_array_member, optional_max_string_member, parse,
    string_member,
};
use talaria_error::{house_error, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

// The catch ladder: not_connected is a state; a scope refusal wants one
// reconnect; everything else is Calendar's own 502 noun.
fn fail(e: CalendarError) -> Response {
    google_fail_with(GoogleError::from(e), "Calendar", "calendar_error")
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    Ok(
        match list_upcoming_events(&state.pg, &sb, &user.id, now_ms(), 10).await {
            Ok(events) => Json(json!({ "events": events })).into_response(),
            Err(e) => fail(e),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let input = match draft(obj) {
        Ok(i) => i,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    Ok(
        match create_event(&state.pg, &sb, &user.id, now_ms(), &input.as_input()).await {
            Ok(event) => Json(json!({ "event": event })).into_response(),
            Err(e) => google_fail_with(e, "Calendar", "calendar_error"),
        },
    )
}

/// The create body as the engine's input (summary/description/location/start/
/// end/allDay/attendees, checked in that order). Shared shape with the agent
/// calendar draft — same schema, different consumer.
fn draft(obj: &serde_json::Map<String, serde_json::Value>) -> Result<EventDraft, String> {
    Ok(EventDraft {
        summary: string_member(obj, "summary", 1, 500)?,
        description: optional_max_string_member(obj, "description", 8000)?,
        location: optional_max_string_member(obj, "location", 500)?,
        start: string_member(obj, "start", 4, usize::MAX)?,
        end: string_member(obj, "end", 4, usize::MAX)?,
        all_day: optional_boolean_member(obj, "allDay")?.unwrap_or(false),
        attendees: optional_email_array_member(obj, "attendees", 50)?.unwrap_or_default(),
    })
}

/// The draft's validated fields, held until the borrow on `obj` ends.
struct EventDraft {
    summary: String,
    description: Option<String>,
    location: Option<String>,
    start: String,
    end: String,
    all_day: bool,
    attendees: Vec<String>,
}

impl EventDraft {
    fn as_input(&self) -> CreateEventInput<'_> {
        CreateEventInput {
            summary: &self.summary,
            description: self.description.as_deref(),
            location: self.location.as_deref(),
            start: &self.start,
            end: &self.end,
            all_day: self.all_day,
            attendees: self.attendees.clone(),
            meet: false,
        }
    }
}

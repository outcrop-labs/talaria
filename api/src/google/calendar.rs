// Google Calendar — port of ui/src/server/google/calendar.ts: read the
// connected identity's upcoming agenda and create events, acting strictly as
// that identity (per-user OAuth, or the org account through an
// already-resolved token).
//
// The connection door is the shared one (crate::google::connections::get_access_token):
// `NotConnected` is a normal answer the brief renders as "no calendar section",
// never an error row.

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::provider::http;
use crate::google::connections::get_access_token;
use crate::google::errors::GoogleError;
use crate::google::oauth::encode_uri_component;
use crate::secretbox::SecretBox;
use serde_json::Value;
use sqlx::PgPool;

/// One agenda entry — wire order pinned (id, summary, start, end, allDay,
/// location, htmlLink, attendees).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub summary: String,
    /// RFC3339 dateTime, or a date (all-day).
    pub start: Option<String>,
    pub end: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
    pub html_link: Option<String>,
    pub attendees: Vec<String>,
}

/// Why the agenda read produced nothing. `NotConnected` is a state, not a
/// failure — the brief skips the schedule section quietly. `Failed` carries the
/// sentence the unreadable-calendar entry quotes.
#[derive(Debug)]
pub enum CalendarError {
    NotConnected,
    Failed(String),
}

impl std::fmt::Display for CalendarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The words matter: the brief decides connected-vs-unreadable by
            // substring over this sentence (the TS regex, on the same words).
            CalendarError::NotConnected => write!(f, "not connected"),
            CalendarError::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// The exact URLSearchParams serialization TS sends (parameter order included),
/// so the request is byte-identical to the one the TS brief issued.
/// `calendar_id` empty/None reads 'primary' (calendar.ts eventsUrl).
fn events_url_with_params(calendar_id: Option<&str>, now_ms: i64, max_results: usize) -> String {
    let wanted = max_results.clamp(1, 50);
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params
        .append_pair("timeMin", &epoch_ms_to_iso(now_ms))
        // Over-fetch 3× so dropped working locations don't shrink the agenda —
        // they were fetched, they just don't count against the slots.
        .append_pair("maxResults", &wanted.saturating_mul(3).min(50).to_string())
        .append_pair("singleEvents", "true")
        .append_pair("orderBy", "startTime");
    let cal = encode_uri_component(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    format!(
        "https://www.googleapis.com/calendar/v3/calendars/{cal}/events?{}",
        params.finish()
    )
}

/// Upcoming events (from now), soonest first.
pub async fn list_upcoming_events(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
    max_results: usize,
) -> Result<Vec<CalendarEvent>, CalendarError> {
    let Some(token) = get_access_token(pg, sb, user_id, now_ms)
        .await
        .map_err(|e| CalendarError::Failed(e.to_string()))?
    else {
        return Err(CalendarError::NotConnected);
    };
    list_upcoming_events_with_token(&token, now_ms, max_results, None).await
}

pub async fn list_upcoming_events_with_token(
    token: &str,
    now_ms: i64,
    max_results: usize,
    calendar_id: Option<&str>,
) -> Result<Vec<CalendarEvent>, CalendarError> {
    let res = http()
        .get(events_url_with_params(calendar_id, now_ms, max_results))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| CalendarError::Failed(format!("calendar list request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(CalendarError::Failed(format!(
            "calendar list failed: {status} {text}"
        )));
    }
    let data: serde_json::Value = res
        .json()
        .await
        .map_err(|e| CalendarError::Failed(format!("calendar list body: {e}")))?;
    let Some(items) = data.get("items").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let wanted = max_results.clamp(1, 50);
    Ok(items
        .iter()
        // Working locations ("at the office Mon–Fri") are Calendar events under
        // the hood — eventType 'workingLocation' — and they repeat all day
        // every weekday, so with singleEvents expansion one of them eats a big
        // share of a 10-slot agenda. They are where you'll be, not what you're
        // doing: an agenda lists commitments. focusTime and outOfOffice stay —
        // those ARE commitments.
        .filter(|e| {
            matches!(
                e.get("eventType")
                    .and_then(|t| t.as_str())
                    .unwrap_or("default"),
                "default" | "focusTime" | "outOfOffice"
            )
        })
        .take(wanted)
        .filter_map(normalize)
        .collect())
}

/// Fold one Google event to `CalendarEvent`; an event with no id is not an
/// event (TS would have crashed indexing it — same outcome, said instead).
fn normalize(e: &serde_json::Value) -> Option<CalendarEvent> {
    let id = e.get("id").and_then(|v| v.as_str())?.to_string();
    let start_obj = e.get("start").filter(|v| v.is_object());
    let end_obj = e.get("end").filter(|v| v.is_object());
    let field = |o: Option<&serde_json::Value>, k: &str| {
        o.and_then(|o| o.get(k))
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let start_date = field(start_obj, "date");
    let start_date_time = field(start_obj, "dateTime");
    let all_day = start_date.is_some() && start_date_time.is_none();
    Some(CalendarEvent {
        id,
        summary: e
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("(no title)")
            .to_string(),
        // `dateTime ?? date ?? null`
        start: start_date_time.clone().or(start_date),
        end: field(end_obj, "dateTime").or_else(|| field(end_obj, "date")),
        all_day,
        location: e.get("location").and_then(|v| v.as_str()).map(String::from),
        html_link: e.get("htmlLink").and_then(|v| v.as_str()).map(String::from),
        attendees: e
            .get("attendees")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|a| a.get("email").and_then(|v| v.as_str()))
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

// ── Create (calendar.ts createEvent) ─────────────────────────────────────────

/// CreateEventInput — the route body's fields, validation already applied.
pub struct CreateEventInput<'a> {
    pub summary: &'a str,
    pub description: Option<&'a str>,
    pub location: Option<&'a str>,
    /// RFC3339 dateTime (timed) or YYYY-MM-DD (all-day).
    pub start: &'a str,
    pub end: &'a str,
    pub all_day: bool,
    pub attendees: Vec<String>,
}

/// Create an event on the user's primary calendar.
pub async fn create_event(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
    input: &CreateEventInput<'_>,
) -> Result<CalendarEvent, GoogleError> {
    let token = crate::google::connections::require_token(pg, sb, user_id, now_ms)
        .await
        .map_err(GoogleError::from)?;
    create_event_with_token(&token, input, None).await
}

/// Create an event using an already-resolved token (per-user or org).
/// `calendar_id` empty/None targets 'primary'. sendUpdates=all: a created
/// event that never told its attendees is a meeting nobody comes to.
pub async fn create_event_with_token(
    token: &str,
    input: &CreateEventInput<'_>,
    calendar_id: Option<&str>,
) -> Result<CalendarEvent, GoogleError> {
    // allDay → date, else dateTime; the same field name on both ends. The
    // optional fields ride only when present — TS's JSON.stringify drops
    // undefined keys, and the body should read like the TS one on the wire.
    let time_field = if input.all_day { "date" } else { "dateTime" };
    let time_obj = |v: &str| {
        serde_json::Map::from_iter([(
            time_field.to_string(),
            serde_json::Value::String(v.to_string()),
        )])
    };
    let mut body = serde_json::Map::new();
    body.insert("summary".into(), serde_json::json!(input.summary));
    if let Some(d) = input.description {
        body.insert("description".into(), serde_json::json!(d));
    }
    if let Some(l) = input.location {
        body.insert("location".into(), serde_json::json!(l));
    }
    body.insert("start".into(), Value::Object(time_obj(input.start)));
    body.insert("end".into(), Value::Object(time_obj(input.end)));
    body.insert(
        "attendees".into(),
        Value::Array(
            input
                .attendees
                .iter()
                .map(|email| serde_json::json!({ "email": email }))
                .collect(),
        ),
    );
    let body = Value::Object(body);
    let cal = encode_uri_component(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    let res = http()
        .post(format!(
            "https://www.googleapis.com/calendar/v3/calendars/{cal}/events?sendUpdates=all"
        ))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("calendar create request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "calendar create failed: {status} {text}"
        )));
    }
    let created: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("calendar create body: {e}")))?;
    Ok(normalize(&created).unwrap_or(CalendarEvent {
        id: String::new(),
        summary: String::new(),
        start: None,
        end: None,
        all_day: false,
        location: None,
        html_link: None,
        attendees: vec![],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_the_shapes_google_actually_returns() {
        let timed = json!({
            "id": "e1", "summary": "Standup",
            "start": {"dateTime": "2026-08-29T14:00:00-04:00"},
            "end": {"dateTime": "2026-08-29T14:30:00-04:00"},
            "location": "Room 5", "htmlLink": "https://cal/e1",
            "attendees": [{"email": "a@x.io"}, {}, {"email": ""}, {"email": "b@x.io"}]
        });
        let e = normalize(&timed).expect("timed normalizes");
        assert_eq!(
            (e.all_day, e.start.as_deref(), e.end.as_deref()),
            (
                false,
                Some("2026-08-29T14:00:00-04:00"),
                Some("2026-08-29T14:30:00-04:00")
            )
        );
        assert_eq!(e.attendees, ["a@x.io", "b@x.io"]);

        let all_day = json!({
            "id": "e2",
            "start": {"date": "2026-08-30"}, "end": {"date": "2026-08-31"}
        });
        let e = normalize(&all_day).expect("all-day normalizes");
        // No summary → '(no title)', not a crash and not a blank line.
        assert_eq!(e.summary, "(no title)");
        assert!(e.all_day);
        assert_eq!(e.start.as_deref(), Some("2026-08-30"));

        // An event with a date AND a dateTime (Google sends both for some
        // recurring edges) is timed.
        let both = json!({"id": "e3", "start": {"date": "2026-08-30", "dateTime": "2026-08-30T09:00:00Z"}});
        assert!(!normalize(&both).expect("both normalizes").all_day);

        assert!(normalize(&json!({"summary": "no id"})).is_none());
    }

    #[test]
    fn working_locations_are_filtered_but_focus_time_stays() {
        let items = json!([
            {"id": "1", "eventType": "workingLocation", "start": {"dateTime": "2026-08-29T09:00:00Z"}},
            {"id": "2", "start": {"dateTime": "2026-08-29T10:00:00Z"}},
            {"id": "3", "eventType": "focusTime", "start": {"dateTime": "2026-08-29T11:00:00Z"}},
            {"id": "4", "eventType": "outOfOffice", "start": {"dateTime": "2026-08-29T12:00:00Z"}},
            {"id": "5", "eventType": "birthday", "start": {"dateTime": "2026-08-29T13:00:00Z"}}
        ]);
        let kept: Vec<&str> = items
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| {
                matches!(
                    e.get("eventType")
                        .and_then(|t| t.as_str())
                        .unwrap_or("default"),
                    "default" | "focusTime" | "outOfOffice"
                )
            })
            .filter_map(|e| e.get("id").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(kept, ["2", "3", "4"]);
    }

    #[test]
    fn the_request_url_is_the_ts_serialization() {
        // Parameter order and values exactly as calendar.ts's URLSearchParams:
        // timeMin first, over-fetched maxResults, singleEvents, orderBy.
        assert_eq!(
            events_url_with_params(None, 1_788_045_420_000, 12),
            "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin=2026-08-29T23%3A17%3A00.000Z&maxResults=36&singleEvents=true&orderBy=startTime"
        );
        // The over-fetch saturates at Google's own 50 ceiling.
        assert!(
            events_url_with_params(None, 0, 50)
                .ends_with("maxResults=50&singleEvents=true&orderBy=startTime")
        );
        // A named calendar rides in the path, encoded — never a query param.
        assert!(
            events_url_with_params(Some("outcrop.co.uk_av1@group.calendar.google.com"), 0, 10)
                .starts_with(
                    "https://www.googleapis.com/calendar/v3/calendars/outcrop.co.uk_av1%40group.calendar.google.com/events?"
                )
        );
    }
}

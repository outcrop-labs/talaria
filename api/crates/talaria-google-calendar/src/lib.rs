// Google Calendar: read the connected identity's upcoming agenda and create
// events, acting strictly as that identity (per-user OAuth, or the org
// account through an already-resolved token).
//
// The connection door is the shared one (talaria_google_connections::get_access_token):
// `NotConnected` is a normal answer the brief renders as "no calendar section",
// never an error row.

use serde_json::Value;
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_body::percent_encode;
use talaria_gateway::provider::http;
use talaria_google_connections::get_access_token;
use talaria_google_errors::GoogleError;
use talaria_secretbox::SecretBox;

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
    /// Present when a Meet link was provisioned. Absent is an honest miss.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hangout_link: Option<String>,
}

/// Why the agenda read produced nothing. `NotConnected` is a state, not a
/// failure — the brief skips the schedule section quietly. `Failed` carries the
/// sentence the unreadable-calendar entry quotes.
#[derive(Debug)]
pub enum CalendarError {
    NotConnected,
    Failed(String),
}

impl From<CalendarError> for GoogleError {
    fn from(e: CalendarError) -> Self {
        match e {
            CalendarError::NotConnected => GoogleError::NotConnected,
            CalendarError::Failed(m) => GoogleError::Failed(m),
        }
    }
}

impl std::fmt::Display for CalendarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The words matter: the brief decides connected-vs-unreadable by
            // substring over this sentence.
            CalendarError::NotConnected => write!(f, "not connected"),
            CalendarError::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// The events-list URL, parameter order pinned (timeMin, maxResults,
/// singleEvents, orderBy). `calendar_id` empty/None reads 'primary'.
/// `time_max` is omitted when absent so the no-window URL stays byte-identical.
pub fn events_url_window(
    calendar_id: Option<&str>,
    time_min: &str,
    time_max: Option<&str>,
    max_results: usize,
) -> String {
    let wanted = max_results.clamp(1, 50);
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("timeMin", time_min);
    if let Some(max) = time_max.filter(|s| !s.is_empty()) {
        params.append_pair("timeMax", max);
    }
    params
        .append_pair("maxResults", &wanted.saturating_mul(3).min(50).to_string())
        .append_pair("singleEvents", "true")
        .append_pair("orderBy", "startTime");
    let cal = percent_encode(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    format!(
        "https://www.googleapis.com/calendar/v3/calendars/{cal}/events?{}",
        params.finish()
    )
}

fn events_url_with_params(calendar_id: Option<&str>, now_ms: i64, max_results: usize) -> String {
    events_url_window(calendar_id, &epoch_ms_to_iso(now_ms), None, max_results)
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

/// Agenda inside an explicit window. `time_min` and `time_max` are already
/// RFC3339 or YYYY-MM-DD — the route validated them. `time_max` absent means
/// open-ended, same as the upcoming read.
pub async fn list_events_in_window_with_token(
    token: &str,
    calendar_id: Option<&str>,
    time_min: &str,
    time_max: Option<&str>,
    max_results: usize,
) -> Result<Vec<CalendarEvent>, CalendarError> {
    let res = http()
        .get(events_url_window(
            calendar_id,
            time_min,
            time_max,
            max_results,
        ))
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
/// event — skipped, not a crash.
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
        // dateTime, else date, else absent
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
        hangout_link: e
            .get("hangoutLink")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
    })
}

// ── Create ───────────────────────────────────────────────────────────────────

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
    /// Ask Google to attach a Meet link. Not guaranteed on insert alone.
    pub meet: bool,
}

/// Create an event on the user's primary calendar.
pub async fn create_event(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
    input: &CreateEventInput<'_>,
) -> Result<CalendarEvent, GoogleError> {
    let token = talaria_google_connections::require_token(pg, sb, user_id, now_ms)
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
    // optional fields ride only when present — absent stays absent in the
    // body.
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
    if input.meet {
        body.insert(
            "conferenceData".into(),
            serde_json::json!({
                "createRequest": {
                    "requestId": format!(
                        "talaria-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    ),
                    "conferenceSolutionKey": { "type": "hangoutsMeet" }
                }
            }),
        );
    }
    let body = Value::Object(body);
    let cal = percent_encode(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    let conference = if input.meet {
        "&conferenceDataVersion=1"
    } else {
        ""
    };
    let res = http()
        .post(format!(
            "https://www.googleapis.com/calendar/v3/calendars/{cal}/events?sendUpdates=all{conference}"
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
    let mut event = normalize(&created).unwrap_or(CalendarEvent {
        id: String::new(),
        summary: String::new(),
        start: None,
        end: None,
        all_day: false,
        location: None,
        html_link: None,
        attendees: vec![],
        hangout_link: None,
    });
    // Meet links are not always on the insert response. One follow-up get,
    // then one more, then the missing link is the honest answer.
    if input.meet && event.hangout_link.is_none() && !event.id.is_empty() {
        for _ in 0..2 {
            let url = format!(
                "https://www.googleapis.com/calendar/v3/calendars/{cal}/events/{}?conferenceDataVersion=1",
                percent_encode(&event.id)
            );
            let got = http().get(url).bearer_auth(token).send().await;
            let Ok(res) = got else { break };
            if !res.status().is_success() {
                break;
            }
            let Ok(body) = res.json::<serde_json::Value>().await else {
                break;
            };
            if let Some(link) = normalize(&body).and_then(|e| e.hangout_link) {
                event.hangout_link = Some(link);
                break;
            }
        }
    }
    Ok(event)
}

/// Patch an existing event. Only fields the caller set are sent. Attendees
/// and time changes notify them (`sendUpdates=all`).
pub async fn update_event_with_token(
    token: &str,
    calendar_id: Option<&str>,
    event_id: &str,
    summary: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    all_day: bool,
) -> Result<CalendarEvent, GoogleError> {
    let mut body = serde_json::Map::new();
    if let Some(s) = summary.filter(|s| !s.is_empty()) {
        body.insert("summary".into(), serde_json::json!(s));
    }
    let time_field = if all_day { "date" } else { "dateTime" };
    if let Some(s) = start.filter(|s| !s.is_empty()) {
        body.insert("start".into(), serde_json::json!({ time_field: s }));
    }
    if let Some(s) = end.filter(|s| !s.is_empty()) {
        body.insert("end".into(), serde_json::json!({ time_field: s }));
    }
    let cal = percent_encode(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    let id = percent_encode(event_id);
    let res = http()
        .patch(format!(
            "https://www.googleapis.com/calendar/v3/calendars/{cal}/events/{id}?sendUpdates=all"
        ))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(serde_json::Value::Object(body).to_string())
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("calendar update request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "calendar update failed: {status} {text}"
        )));
    }
    let updated: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("calendar update body: {e}")))?;
    Ok(normalize(&updated).unwrap_or(CalendarEvent {
        id: event_id.to_string(),
        summary: String::new(),
        start: None,
        end: None,
        all_day: false,
        location: None,
        html_link: None,
        attendees: vec![],
        hangout_link: None,
    }))
}

/// Cancel an event and tell attendees. Delete is the Calendar cancel.
pub async fn cancel_event_with_token(
    token: &str,
    calendar_id: Option<&str>,
    event_id: &str,
) -> Result<(), GoogleError> {
    let cal = percent_encode(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    let id = percent_encode(event_id);
    let res = http()
        .delete(format!(
            "https://www.googleapis.com/calendar/v3/calendars/{cal}/events/{id}?sendUpdates=all"
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("calendar cancel request: {e}")))?;
    if !res.status().is_success() && res.status().as_u16() != 404 {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "calendar cancel failed: {status} {text}"
        )));
    }
    Ok(())
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
    fn the_events_url_is_pinned_parameter_for_parameter() {
        // Parameter order and values: timeMin first, over-fetched maxResults,
        // singleEvents, orderBy.
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
        assert!(
            events_url_window(
                None,
                "2026-09-24T00:00:00.000Z",
                Some("2026-09-25T00:00:00.000Z"),
                10
            )
            .contains("timeMax=2026-09-25T00%3A00%3A00.000Z")
        );
        assert!(
            !events_url_window(None, "2026-09-24T00:00:00.000Z", None, 10).contains("timeMax=")
        );
    }
}

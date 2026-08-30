// Google Calendar read — the agenda half of
// ui/src/server/google/calendar.ts (listUpcomingEvents + normalize + the event
// type filter). The create-event leg ports with the integrations plane; the
// brief only ever reads.
//
// The connection door is the shared one (google_connections::get_access_token):
// `NotConnected` is a normal answer the brief renders as "no calendar section",
// never an error row.

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::provider::http;
use crate::google_connections::get_access_token;
use crate::secretbox::SecretBox;
use sqlx::PgPool;

/// One agenda entry, folded to the fields the brief reads.
#[derive(Debug, Clone)]
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
fn events_url_with_params(now_ms: i64, max_results: usize) -> String {
    let wanted = max_results.clamp(1, 50);
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params
        .append_pair("timeMin", &epoch_ms_to_iso(now_ms))
        // Over-fetch 3× so dropped working locations don't shrink the agenda —
        // they were fetched, they just don't count against the slots.
        .append_pair("maxResults", &wanted.saturating_mul(3).min(50).to_string())
        .append_pair("singleEvents", "true")
        .append_pair("orderBy", "startTime");
    format!(
        "https://www.googleapis.com/calendar/v3/calendars/primary/events?{}",
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
    list_upcoming_events_with_token(&token, now_ms, max_results).await
}

pub async fn list_upcoming_events_with_token(
    token: &str,
    now_ms: i64,
    max_results: usize,
) -> Result<Vec<CalendarEvent>, CalendarError> {
    let res = http()
        .get(events_url_with_params(now_ms, max_results))
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
            events_url_with_params(1_788_045_420_000, 12),
            "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin=2026-08-29T23%3A17%3A00.000Z&maxResults=36&singleEvents=true&orderBy=startTime"
        );
        // The over-fetch saturates at Google's own 50 ceiling.
        assert!(
            events_url_with_params(0, 50)
                .ends_with("maxResults=50&singleEvents=true&orderBy=startTime")
        );
    }
}

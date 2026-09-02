// google/errors.ts — the one mapping every Google service route answers
// through. The engine halves speak `GoogleError` (TS speaks a thrown Error
// with `name: 'GoogleNotConnected'`; the enum is the same distinction said
// in the type instead of the name field), and this leaf turns one into the
// consistent API response: `surface` names the product (Drive, Calendar,
// Gmail, APIs) for the reconnect/unavailable sentences.

use axum::http::StatusCode;
use axum::response::Response;

use crate::error::house_error_msg;

/// A Google service call's failure. `NotConnected` is requireToken's
/// GoogleNotConnected throw — a state, not an outage. `Failed` carries the
/// sentence Google sent (the log's only consumer).
#[derive(Debug)]
pub enum GoogleError {
    NotConnected,
    Failed(String),
}

impl std::fmt::Display for GoogleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoogleError::NotConnected => write!(f, "not_connected"),
            GoogleError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl From<crate::google::connections::RequireError> for GoogleError {
    fn from(e: crate::google::connections::RequireError) -> Self {
        match e {
            crate::google::connections::RequireError::NotConnected => GoogleError::NotConnected,
            crate::google::connections::RequireError::Failed(m) => GoogleError::Failed(m),
        }
    }
}

/// calendar.ts's error type reaching a googleFail boundary: its NotConnected
/// is the same state googleFail answers, and its Failed sentence is the
/// message the reconnect test reads.
impl From<crate::google::calendar::CalendarError> for GoogleError {
    fn from(e: crate::google::calendar::CalendarError) -> Self {
        match e {
            crate::google::calendar::CalendarError::NotConnected => GoogleError::NotConnected,
            crate::google::calendar::CalendarError::Failed(m) => GoogleError::Failed(m),
        }
    }
}

/// The scope-refusal test every surface shares (TS `/insufficient|ACCESS_TOKEN_SCOPE/i`
/// over the message): Google says this in several wordings when the connection
/// predates a scope the surface needs, and the fix is the same every time —
/// one reconnect.
pub fn reconnect_needed(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("insufficient") || m.contains("access_token_scope")
}

/// googleFail — the default mapping (502 code `google_error`).
pub fn google_fail(e: GoogleError, surface: &str) -> Response {
    google_fail_with(e, surface, "google_error")
}

/// googleFail with the 502's error code named by the caller: the calendar
/// route answers `calendar_error`, the drive routes `drive_error` /
/// `import_failed` — same ladder, their own noun.
pub fn google_fail_with(e: GoogleError, surface: &str, code: &'static str) -> Response {
    match e {
        GoogleError::NotConnected => house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "Connect a Google account first.",
        ),
        GoogleError::Failed(m) if reconnect_needed(&m) => house_error_msg(
            StatusCode::CONFLICT,
            "reconnect_needed",
            &format!("Reconnect Google to grant {surface} access."),
        ),
        GoogleError::Failed(m) => {
            tracing::error!("[google/{}] failed: {}", surface.to_lowercase(), m);
            house_error_msg(
                StatusCode::BAD_GATEWAY,
                code,
                &format!("Could not reach Google {surface}."),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reconnect_regex_is_the_ts_one() {
        // Case-insensitive over the message, both spellings.
        assert!(reconnect_needed(
            "Google answered 403: Request had insufficient authentication scopes."
        ));
        assert!(reconnect_needed("ACCESS_TOKEN_SCOPE: the grant lacks it"));
        assert!(reconnect_needed("access_token_scope lowercase"));
        assert!(!reconnect_needed("calendar list failed: 500 boom"));
        // 'insufficient' as a substring of any wording counts.
        assert!(reconnect_needed("Insufficient Permission"));
    }
}

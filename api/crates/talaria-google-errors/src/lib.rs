// google/errors — the one mapping every Google service route answers through.
// The engine halves speak `GoogleError`, and this leaf turns one into the
// consistent API response: `surface` names the product (Drive, Calendar,
// Gmail, APIs) for the reconnect/unavailable sentences.
//
// Google's own words reach the caller. A failure message carries Google's
// status and body verbatim (`<what> failed: <status> <body>`), and
// `upstream_reason` lifts the sentence Google wrote out of it. The answer
// quotes that sentence; it never rewrites it, and nothing here guesses a
// class of refusal from wording Google has not been seen to send.

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;

use talaria_error::house_error_msg;
use talaria_google_connections::TokenError;

/// A Google service call's failure. `NotConnected` is a state, not an outage.
/// `Failed` carries the sentence Google sent — logged, and quoted back.
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

impl From<talaria_google_connections::RequireError> for GoogleError {
    fn from(e: talaria_google_connections::RequireError) -> Self {
        match e {
            talaria_google_connections::RequireError::NotConnected => GoogleError::NotConnected,
            talaria_google_connections::RequireError::Failed(m) => GoogleError::Failed(m),
        }
    }
}

/// The longest quote an answer carries — the provisioning precedent.
pub const REASON_MAX_CHARS: usize = 300;

/// Google's error body inside a failure message: the first `{` from which a
/// JSON object with an `error` member parses. Every engine formats its
/// failure as `<what> failed: <status> <body>`, so the body is the tail.
fn google_error_body(message: &str) -> Option<Value> {
    message.match_indices('{').find_map(|(at, _)| {
        let mut stream = serde_json::Deserializer::from_str(&message[at..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(v)) if v.get("error").is_some() => Some(v),
            _ => None,
        }
    })
}

/// What Google said, in its own words, from a failure message — or None when
/// the message holds no Google error body (a transport failure, a local
/// fault: nothing Google wrote to quote).
///
/// Two shapes:
/// - REST (`{"error": {"code", "message", "status", "details"}}`): the
///   message, led by the most specific code Google gave — the `ErrorInfo`
///   reason (`SERVICE_DISABLED`), else `status` (`PERMISSION_DENIED`), else
///   the first legacy `errors[].reason` (Drive v3 sends only that).
/// - OAuth (`{"error": "invalid_grant", "error_description": …}`): the code,
///   then the description.
///
/// Whitespace is folded and the whole is capped at `REASON_MAX_CHARS`, with
/// an ellipsis when cut — the code leads, so a cut never loses it.
pub fn upstream_reason(message: &str) -> Option<String> {
    let body = google_error_body(message)?;
    let error = body.get("error")?;
    let text = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let (code, said) = match error {
        Value::Object(rest) => {
            let info_reason = rest
                .get("details")
                .and_then(Value::as_array)
                .and_then(|details| {
                    details.iter().find_map(|d| {
                        let is_info = d
                            .get("@type")
                            .and_then(Value::as_str)
                            .is_some_and(|t| t.ends_with("google.rpc.ErrorInfo"));
                        if is_info { text(d.get("reason")) } else { None }
                    })
                });
            let legacy_reason = rest
                .get("errors")
                .and_then(Value::as_array)
                .and_then(|errors| errors.first())
                .and_then(|first| text(first.get("reason")));
            let code = info_reason
                .or_else(|| text(rest.get("status")))
                .or(legacy_reason);
            (code, text(rest.get("message")))
        }
        Value::String(_) => (text(Some(error)), text(body.get("error_description"))),
        _ => return None,
    };
    let joined = match (code, said) {
        (Some(code), Some(said)) => format!("{code}: {said}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => return None,
    };
    Some(cap(&fold_whitespace(&joined)))
}

/// Google's OAuth error CODE (`invalid_client`, `access_denied`) from a
/// failure message or a bare value — only when it is the snake_case word
/// RFC 6749 §5.2 and §4.1.2.1 define. The connect bounce carries it in a
/// URL anyone can craft, so the landing page only ever renders a code, never
/// free text.
pub fn oauth_error_code(message: &str) -> Option<String> {
    let code = match google_error_body(message) {
        Some(body) => body.get("error")?.as_str()?.to_string(),
        None => message.trim().to_string(),
    };
    is_oauth_code(&code).then_some(code)
}

fn is_oauth_code(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn fold_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cap(s: &str) -> String {
    if s.chars().count() <= REASON_MAX_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(REASON_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

/// The scope-refusal test every surface shares (case-insensitive
/// "insufficient" or "access_token_scope" in the message): Google says this in
/// several wordings when the connection
/// predates a scope the surface needs, and the fix is the same every time —
/// one reconnect.
pub fn reconnect_needed(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("insufficient") || m.contains("access_token_scope")
}

/// The default mapping (502 code `google_error`).
pub fn google_fail(e: GoogleError, surface: &str) -> Response {
    google_fail_with(e, surface, "google_error")
}

/// google_fail with the 502's error code named by the caller: the calendar
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
            &reconnect_sentence(surface, upstream_reason(&m).as_deref()),
        ),
        GoogleError::Failed(m) => {
            tracing::error!("[google/{}] failed: {}", surface.to_lowercase(), m);
            house_error_msg(
                StatusCode::BAD_GATEWAY,
                code,
                &refused_sentence(surface, upstream_reason(&m).as_deref()),
            )
        }
    }
}

/// The scope-reconnect sentence, with Google's words when it sent any.
pub fn reconnect_sentence(surface: &str, reason: Option<&str>) -> String {
    match reason {
        Some(r) => format!("Reconnect Google to grant {surface} access. Google said: «{r}»"),
        None => format!("Reconnect Google to grant {surface} access."),
    }
}

/// The 502 sentence: Google's words when it answered, the reach failure
/// when nothing Google wrote came back.
pub fn refused_sentence(surface: &str, reason: Option<&str>) -> String {
    match reason {
        Some(r) => format!("Google {surface} refused: «{r}»"),
        None => format!("Could not reach Google {surface}."),
    }
}

/// An access token that could not be had, as the route answers it. This is
/// the door agent routes go through when the CONNECTION, not the product
/// call, failed — before this, every such failure read "not connected".
///
/// - `InvalidGrant` → 409 `reconnect_needed`: Google refused the stored
///   grant, the vend already cleared it, and a reconnect is the fix.
/// - `ClientRejected` → 502 `client_rejected`: Google refused this server's
///   OAuth client; the connection was kept and an admin owns the fix.
/// - `Other` → 502 `google_error`, quoting Google when it answered.
pub fn google_token_fail(e: TokenError, surface: &str) -> Response {
    let reason = upstream_reason(&e.to_string());
    match e {
        TokenError::InvalidGrant(m) => {
            tracing::warn!("[google/{}] token refused: {}", surface.to_lowercase(), m);
            house_error_msg(
                StatusCode::CONFLICT,
                "reconnect_needed",
                &match reason {
                    Some(r) => format!(
                        "Google refused the stored Google connection, so it was cleared: «{r}». Reconnect it to restore {surface} access."
                    ),
                    None => format!(
                        "Google refused the stored Google connection, so it was cleared. Reconnect it to restore {surface} access."
                    ),
                },
            )
        }
        TokenError::ClientRejected(m) => {
            tracing::error!("[google/{}] client rejected: {}", surface.to_lowercase(), m);
            house_error_msg(
                StatusCode::BAD_GATEWAY,
                "client_rejected",
                &match reason {
                    Some(r) => format!(
                        "Google refused this server's OAuth client: «{r}». The connection was kept; an admin needs to check the client under Admin → Google Workspace."
                    ),
                    None => "Google refused this server's OAuth client. The connection was kept; an admin needs to check the client under Admin → Google Workspace.".to_string(),
                },
            )
        }
        TokenError::Other(m) => {
            tracing::error!("[google/{}] token failed: {}", surface.to_lowercase(), m);
            house_error_msg(
                StatusCode::BAD_GATEWAY,
                "google_error",
                &match reason {
                    Some(r) => format!("Google refused the token refresh: «{r}»"),
                    None => format!("Could not get a Google token for {surface}."),
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Captured bodies — never invented ────────────────────────────────────
    //
    // From the outcrop-labs instance's app log, 2026-11-12, read-only agent
    // GETs of ids that do not exist (the `[google/*] failed:` lines this
    // crate logs). The project number is the instance's own, kept verbatim.
    const DOCS_404: &str = r#"docs get failed: 404 Not Found {
  "error": {
    "code": 404,
    "message": "File not found: talariaCaptureNonexistentDoc0000000000000.",
    "errors": [
      {
        "message": "File not found: talariaCaptureNonexistentDoc0000000000000.",
        "domain": "global",
        "reason": "notFound",
        "location": "fileId",
        "locationType": "parameter"
      }
    ]
  }
}
"#;
    const SHEETS_DISABLED: &str = r#"sheets meta failed: 403 Forbidden {
  "error": {
    "code": 403,
    "message": "Google Sheets API has not been used in project 95159671789 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=95159671789 then retry. If you enabled this API recently, wait a few minutes for the action to propagate to our systems and retry.",
    "status": "PERMISSION_DENIED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": "SERVICE_DISABLED",
        "domain": "googleapis.com",
        "metadata": {
          "serviceTitle": "Google Sheets API",
          "activationUrl": "https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=95159671789",
          "containerInfo": "95159671789",
          "consumer": "projects/95159671789",
          "service": "sheets.googleapis.com"
        }
      },
      {
        "@type": "type.googleapis.com/google.rpc.LocalizedMessage",
        "locale": "en-US",
        "message": "Google Sheets API has not been used in project 95159671789 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=95159671789 then retry. If you enabled this API recently, wait a few minutes for the action to propagate to our systems and retry."
      },
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Google developers console API activation",
            "url": "https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=95159671789"
          }
        ]
      }
    ]
  }
}
"#;
    // From gmail.googleapis.com directly, 2026-11-12: no credential, and a
    // bearer that is not a token.
    const GMAIL_NO_AUTH: &str = r#"gmail labels failed: 401 Unauthorized {
  "error": {
    "code": 401,
    "message": "Request is missing required authentication credential. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.",
    "errors": [
      {
        "message": "Login Required.",
        "domain": "global",
        "reason": "required",
        "location": "Authorization",
        "locationType": "header"
      }
    ],
    "status": "UNAUTHENTICATED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": "CREDENTIALS_MISSING",
        "domain": "googleapis.com",
        "metadata": {
          "method": "caribou.api.proto.MailboxService.ListLabels",
          "service": "gmail.googleapis.com"
        }
      }
    ]
  }
}
"#;
    const GMAIL_BAD_BEARER: &str = r#"gmail labels failed: 401 Unauthorized {
  "error": {
    "code": 401,
    "message": "Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.",
    "errors": [
      {
        "message": "Invalid Credentials",
        "domain": "global",
        "reason": "authError",
        "location": "Authorization",
        "locationType": "header"
      }
    ],
    "status": "UNAUTHENTICATED"
  }
}
"#;
    // From oauth2.googleapis.com/token, 2026-11-12, with no real credential
    // (see talaria-google-connections' tests for how each was provoked).
    const TOKEN_CLIENT_NOT_FOUND: &str = "google token refresh failed: 401 Unauthorized {\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The OAuth client was not found.\"\n}";
    const TOKEN_BAD_SECRET: &str = "google token refresh failed: 401 Unauthorized {\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The provided client secret is invalid.\"\n}";
    const TOKEN_BAD_REFRESH: &str = "google token refresh failed: 400 Bad Request {\n  \"error\": \"invalid_grant\",\n  \"error_description\": \"Bad Request\"\n}";

    #[test]
    fn the_reconnect_regex_matches_both_spellings() {
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

    #[test]
    fn a_rest_refusal_is_quoted_with_its_most_specific_code() {
        assert_eq!(
            upstream_reason(DOCS_404).as_deref(),
            Some("notFound: File not found: talariaCaptureNonexistentDoc0000000000000.")
        );
        assert_eq!(
            upstream_reason(GMAIL_NO_AUTH).as_deref(),
            Some(
                "CREDENTIALS_MISSING: Request is missing required authentication credential. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project."
            )
        );
        assert_eq!(
            upstream_reason(GMAIL_BAD_BEARER).as_deref(),
            Some(
                "UNAUTHENTICATED: Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project."
            )
        );
    }

    #[test]
    fn a_disabled_api_keeps_its_code_and_the_enable_link_under_the_cap() {
        let r = upstream_reason(SHEETS_DISABLED).expect("quoted");
        assert!(r.starts_with("SERVICE_DISABLED: Google Sheets API has not been used"));
        assert!(r.contains(
            "https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=95159671789"
        ));
        assert_eq!(r.chars().count(), REASON_MAX_CHARS);
        assert!(r.ends_with('…'));
    }

    #[test]
    fn an_oauth_refusal_is_quoted_code_first() {
        assert_eq!(
            upstream_reason(TOKEN_CLIENT_NOT_FOUND).as_deref(),
            Some("invalid_client: The OAuth client was not found.")
        );
        assert_eq!(
            upstream_reason(TOKEN_BAD_SECRET).as_deref(),
            Some("invalid_client: The provided client secret is invalid.")
        );
        assert_eq!(
            upstream_reason(TOKEN_BAD_REFRESH).as_deref(),
            Some("invalid_grant: Bad Request")
        );
    }

    #[test]
    fn nothing_google_wrote_means_nothing_quoted() {
        assert_eq!(
            upstream_reason("gmail list request: error sending request for url"),
            None
        );
        assert_eq!(
            upstream_reason("Google answered 403: Request had insufficient authentication scopes."),
            None
        );
        // A brace that is not Google's error body is not mistaken for one.
        assert_eq!(upstream_reason("drive path body: {\"files\": []}"), None);
        assert_eq!(upstream_reason("unbalanced { brace"), None);
    }

    #[test]
    fn the_oauth_code_is_a_bare_snake_case_word_or_nothing() {
        assert_eq!(
            oauth_error_code(TOKEN_BAD_SECRET).as_deref(),
            Some("invalid_client")
        );
        assert_eq!(
            oauth_error_code(TOKEN_BAD_REFRESH).as_deref(),
            Some("invalid_grant")
        );
        // Google's redirect `?error=` value, as the callback reads it.
        assert_eq!(
            oauth_error_code("access_denied").as_deref(),
            Some("access_denied")
        );
        // A REST body's `error` is an object, not a code.
        assert_eq!(oauth_error_code(DOCS_404), None);
        // Free text never rides the bounce.
        assert_eq!(oauth_error_code("Call 555-0100 to fix this"), None);
        assert_eq!(oauth_error_code(""), None);
        assert_eq!(oauth_error_code(&"a".repeat(65)), None);
    }

    async fn body_of(res: Response) -> (StatusCode, Value) {
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).expect("json body"))
    }

    #[tokio::test]
    async fn a_failed_call_answers_googles_words_not_a_reach_failure() {
        let (status, body) = body_of(google_fail(
            GoogleError::Failed(SHEETS_DISABLED.into()),
            "Sheets",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "google_error");
        let msg = body["message"].as_str().unwrap();
        assert!(msg.starts_with("Google Sheets refused: «SERVICE_DISABLED: Google Sheets API"));

        // Nothing Google wrote: the old sentence, unchanged.
        let (_, body) = body_of(google_fail(
            GoogleError::Failed("gmail list request: timed out".into()),
            "Gmail",
        ))
        .await;
        assert_eq!(body["message"], "Could not reach Google Gmail.");
    }

    #[tokio::test]
    async fn a_scope_refusal_still_asks_for_a_reconnect() {
        let (status, body) = body_of(google_fail(
            GoogleError::Failed(
                "Google answered 403: Request had insufficient authentication scopes.".into(),
            ),
            "Drive",
        ))
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "reconnect_needed");
        assert_eq!(body["message"], "Reconnect Google to grant Drive access.");
        // When Google's body rides along, its words follow the fix.
        assert_eq!(
            reconnect_sentence("Drive", upstream_reason(GMAIL_BAD_BEARER).as_deref()),
            "Reconnect Google to grant Drive access. Google said: «UNAUTHENTICATED: Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.»"
        );
    }

    #[tokio::test]
    async fn a_token_failure_is_never_read_as_not_connected() {
        let (status, body) = body_of(google_token_fail(
            TokenError::InvalidGrant(TOKEN_BAD_REFRESH.into()),
            "Gmail",
        ))
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "reconnect_needed");
        assert_eq!(
            body["message"],
            "Google refused the stored Google connection, so it was cleared: «invalid_grant: Bad Request». Reconnect it to restore Gmail access."
        );

        let (status, body) = body_of(google_token_fail(
            TokenError::ClientRejected(TOKEN_BAD_SECRET.into()),
            "Gmail",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "client_rejected");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("Google refused this server's OAuth client: «invalid_client: The provided client secret is invalid.»")
        );

        let (status, body) = body_of(google_token_fail(
            TokenError::Other("no google client configured".into()),
            "Docs",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["message"], "Could not get a Google token for Docs.");
    }
}

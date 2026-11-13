// Google API health — is the Google side actually usable? OAuth consent
// succeeds even when the project's APIs are disabled — Google only checks at
// call time — so a freshly connected org account still 403s on the first Drive
// export / Calendar read / Gmail fetch / Sheets or Slides read until someone
// flips the library switches in Google Cloud Console. This module makes that
// checkable on demand from the Admin UI instead of discovering it one broken
// surface at a time.
//
// GOOGLE_API_LIBRARY itself lives client-side (ui/src/lib/google-apis.ts — the
// Admin UI renders it as setup instructions); the server keeps this mirror
// because the API must not import from a browser bundle.

use serde_json::Value;
use sqlx::PgPool;

use talaria_gateway::provider::http;
use talaria_google_connections::TokenError;
use talaria_google_org::get_org_access_token;
use talaria_secretbox::SecretBox;

/// What a healthy answer looks like for one probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Healthy {
    /// A 2xx: the probe read something real.
    Success,
    /// A 404: the probe asked for an id that cannot exist, and only an
    /// enabled API gets far enough to say "not found". Sheets and Slides
    /// have no cheap list call, so a miss is their cheapest proof of life.
    NotFound,
}

/// One of the services the library table names.
struct LibraryEntry {
    service: &'static str,
    name: &'static str,
    console_url: &'static str,
    probe_url: &'static str,
    healthy: Healthy,
}

/// The id the not-found probes ask for. Never a real document: it is not
/// the shape Google mints.
macro_rules! absent_id {
    () => {
        "talaria-api-health-probe-nonexistent"
    };
}

/// The library (the client-side table mirrored above) joined with each
/// service's probe — one cheapest-possible authenticated call per service,
/// chosen to stay inside the scopes WORKSPACE_SCOPES already grants (no extra
/// consent to test). Every REST API the Google tools call is here: Drive
/// (Docs travel through Drive), Calendar, Gmail, Sheets and Slides.
static GOOGLE_API_LIBRARY: [LibraryEntry; 5] = [
    LibraryEntry {
        service: "drive",
        name: "Google Drive API",
        console_url: "https://console.cloud.google.com/apis/library/drive.googleapis.com",
        // One file of the root listing — drive.readonly grants it, and unlike
        // about.get it has no required params to get wrong.
        probe_url: "https://www.googleapis.com/drive/v3/files?pageSize=1&fields=files(id)",
        healthy: Healthy::Success,
    },
    LibraryEntry {
        service: "calendar",
        name: "Google Calendar API",
        console_url: "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com",
        // One event of the primary calendar — calendar.events grants exactly
        // this.
        probe_url: "https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults=1&fields=nextPageToken",
        healthy: Healthy::Success,
    },
    LibraryEntry {
        service: "gmail",
        name: "Gmail API",
        console_url: "https://console.cloud.google.com/apis/library/gmail.googleapis.com",
        // The Gmail profile (mailbox address + history id) — gmail.modify
        // covers it.
        probe_url: "https://www.googleapis.com/gmail/v1/users/me/profile",
        healthy: Healthy::Success,
    },
    LibraryEntry {
        service: "sheets",
        name: "Google Sheets API",
        console_url: "https://console.cloud.google.com/apis/library/sheets.googleapis.com",
        // spreadsheets.get on an id that cannot exist — the `drive` scope
        // covers it.
        probe_url: concat!(
            "https://sheets.googleapis.com/v4/spreadsheets/",
            absent_id!(),
            "?fields=spreadsheetId"
        ),
        healthy: Healthy::NotFound,
    },
    LibraryEntry {
        service: "slides",
        name: "Google Slides API",
        console_url: "https://console.cloud.google.com/apis/library/slides.googleapis.com",
        // presentations.get on an id that cannot exist — the `drive` scope
        // covers it.
        probe_url: concat!(
            "https://slides.googleapis.com/v1/presentations/",
            absent_id!(),
            "?fields=presentationId"
        ),
        healthy: Healthy::NotFound,
    },
];

/// One probe's answer (GoogleApiHealth) — wire order pinned (service, name,
/// consoleUrl, state, detail).
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleApiHealth {
    pub service: &'static str,
    pub name: &'static str,
    /// Where to enable it: the project-specific page Google named in a
    /// disabled answer, else the library page.
    pub console_url: String,
    /// ok · disabled (enable it in the console) · error (anything else)
    pub state: &'static str,
    /// Google's own sentence for the failing states; null when ok.
    pub detail: Option<String>,
}

/// Google's `error.message`, capped and never reworded. Long enough for
/// Google's whole SERVICE_DISABLED sentence (323 characters as captured),
/// short enough to stay one paragraph.
const DETAIL_CAP: usize = 500;

fn google_message(body: &Value) -> Option<String> {
    body.get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| m.chars().take(DETAIL_CAP).collect())
}

/// Google signals "you never enabled this API" several ways depending on the
/// service's vintage: SERVICE_DISABLED in ErrorInfo details, accessNotConfigured
/// in legacy errors[], or just a sentence about it in the message.
fn is_disabled(status: u16, body: &Value) -> bool {
    if status != 403 && status != 404 {
        return false;
    }
    let Some(e) = body.get("error") else {
        return false;
    };
    let has_reason = |key: &str, reason: &str| {
        e.get(key).and_then(Value::as_array).is_some_and(|list| {
            list.iter()
                .any(|x| x.get("reason").and_then(Value::as_str) == Some(reason))
        })
    };
    if has_reason("details", "SERVICE_DISABLED") {
        return true;
    }
    if has_reason("errors", "accessNotConfigured") {
        return true;
    }
    e.get("message").and_then(Value::as_str).is_some_and(|m| {
        let m = m.to_lowercase();
        m.contains("has not been used in project") || m.contains("it is disabled")
    })
}

/// The activation page Google names in a SERVICE_DISABLED answer — it opens
/// the right project, where the library page opens whichever one the console
/// last had. Only a Google console URL is taken.
fn activation_url(body: &Value) -> Option<String> {
    body.get("error")?
        .get("details")?
        .as_array()?
        .iter()
        .filter_map(|d| d.get("metadata")?.get("activationUrl")?.as_str())
        .find(|u| {
            u.starts_with("https://console.developers.google.com/")
                || u.starts_with("https://console.cloud.google.com/")
        })
        .map(String::from)
}

/// One probe's HTTP answer → its health. Pure, so the classification is
/// proven on Google's captured bodies.
fn classify(entry: &LibraryEntry, status: u16, body: &Value) -> GoogleApiHealth {
    let answer = |state: &'static str, detail: Option<String>, console_url: Option<String>| {
        GoogleApiHealth {
            service: entry.service,
            name: entry.name,
            console_url: console_url.unwrap_or_else(|| entry.console_url.to_string()),
            state,
            detail,
        }
    };
    if (200..300).contains(&status) {
        return answer("ok", None, None);
    }
    if is_disabled(status, body) {
        return answer(
            "disabled",
            Some(
                google_message(body)
                    .unwrap_or_else(|| "Not enabled in this Google Cloud project.".into()),
            ),
            activation_url(body),
        );
    }
    if status == 404 && entry.healthy == Healthy::NotFound {
        return answer("ok", None, None);
    }
    if status == 401 {
        let mut detail = "Google rejected the access token. Reconnect the org account.".to_string();
        if let Some(m) = google_message(body) {
            detail = format!("{detail} Google said: {m}");
        }
        return answer("error", Some(detail), None);
    }
    answer(
        "error",
        Some(google_message(body).unwrap_or_else(|| format!("Google answered {status}."))),
        None,
    )
}

async fn probe(entry: &LibraryEntry, token: &str) -> Result<GoogleApiHealth, String> {
    let res = http()
        .get(entry.probe_url)
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("google health probe request: {e}"))?;
    let status = res.status().as_u16();
    let body: Value = if res.status().is_success() {
        Value::Null
    } else {
        res.json().await.unwrap_or(Value::Null)
    };
    Ok(classify(entry, status, &body))
}

/// Probe every service with the org connection's access token.
/// `Err(NotConnected)` means the org connection is dead — the route maps it
/// to a reconnect prompt; `Err(Failed)` is the token read itself blowing up
/// (InvalidGrant etc.).
pub async fn probe_org_google_apis(
    pg: &PgPool,
    sb: &SecretBox,
    now_ms: i64,
) -> Result<Vec<GoogleApiHealth>, talaria_google_errors::GoogleError> {
    let token = get_org_access_token(pg, sb, now_ms)
        .await
        .map_err(|e: TokenError| talaria_google_errors::GoogleError::Failed(e.to_string()))?;
    let Some(token) = token else {
        return Err(talaria_google_errors::GoogleError::NotConnected);
    };
    // Probes run concurrently — five sequential Google round-trips cost
    // +500ms for nothing; join_all keeps the library's order and surfaces the
    // first failure.
    let out: Vec<GoogleApiHealth> =
        futures_util::future::join_all(GOOGLE_API_LIBRARY.iter().map(|entry| probe(entry, &token)))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, String>>()
            .map_err(talaria_google_errors::GoogleError::Failed)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(service: &str) -> &'static LibraryEntry {
        GOOGLE_API_LIBRARY
            .iter()
            .find(|e| e.service == service)
            .expect("a library entry")
    }

    // Captured live on 2026-11-12 from the outcrop-labs instance's org
    // connection (its Google Cloud project has both APIs off), byte for byte
    // except the project number, replaced with zeros. Sheets answered the
    // spreadsheets.get of a nonexistent id, Slides the presentations.get.
    const SHEETS_DISABLED: &str = r#"{
  "error": {
    "code": 403,
    "message": "Google Sheets API has not been used in project 00000000000 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=00000000000 then retry. If you enabled this API recently, wait a few minutes for the action to propagate to our systems and retry.",
    "status": "PERMISSION_DENIED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": "SERVICE_DISABLED",
        "domain": "googleapis.com",
        "metadata": {
          "consumer": "projects/00000000000",
          "containerInfo": "00000000000",
          "serviceTitle": "Google Sheets API",
          "service": "sheets.googleapis.com",
          "activationUrl": "https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=00000000000"
        }
      },
      {
        "@type": "type.googleapis.com/google.rpc.LocalizedMessage",
        "locale": "en-US",
        "message": "Google Sheets API has not been used in project 00000000000 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=00000000000 then retry. If you enabled this API recently, wait a few minutes for the action to propagate to our systems and retry."
      },
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Google developers console API activation",
            "url": "https://console.developers.google.com/apis/api/sheets.googleapis.com/overview?project=00000000000"
          }
        ]
      }
    ]
  }
}"#;
    const SLIDES_DISABLED: &str = r#"{
  "error": {
    "code": 403,
    "message": "Google Slides API has not been used in project 00000000000 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/slides.googleapis.com/overview?project=00000000000 then retry. If you enabled this API recently, wait a few minutes for the action to propagate to our systems and retry.",
    "status": "PERMISSION_DENIED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": "SERVICE_DISABLED",
        "domain": "googleapis.com",
        "metadata": {
          "consumer": "projects/00000000000",
          "serviceTitle": "Google Slides API",
          "activationUrl": "https://console.developers.google.com/apis/api/slides.googleapis.com/overview?project=00000000000",
          "service": "slides.googleapis.com",
          "containerInfo": "00000000000"
        }
      },
      {
        "@type": "type.googleapis.com/google.rpc.LocalizedMessage",
        "locale": "en-US",
        "message": "Google Slides API has not been used in project 00000000000 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/slides.googleapis.com/overview?project=00000000000 then retry. If you enabled this API recently, wait a few minutes for the action to propagate to our systems and retry."
      },
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Google developers console API activation",
            "url": "https://console.developers.google.com/apis/api/slides.googleapis.com/overview?project=00000000000"
          }
        ]
      }
    ]
  }
}"#;
    // Captured the same day from the same connection: Drive files.get of a
    // nonexistent id — an ENABLED API's 404. (Sheets' and Slides' own 404
    // could not be captured there: both APIs are off in that project.)
    const DRIVE_NOT_FOUND: &str = r#"{
  "error": {
    "code": 404,
    "message": "File not found: talaria-api-health-probe-nonexistent.",
    "errors": [
      {
        "message": "File not found: talaria-api-health-probe-nonexistent.",
        "domain": "global",
        "reason": "notFound",
        "location": "fileId",
        "locationType": "parameter"
      }
    ]
  }
}"#;

    // Captured the same day: the Sheets probe URL with a bearer that is not a
    // token → HTTP 401. Drive, Calendar, Gmail and Slides answered the same
    // sentence from the worktree stack's probe.
    const SHEETS_BAD_TOKEN: &str = r#"{
  "error": {
    "code": 401,
    "message": "Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.",
    "status": "UNAUTHENTICATED"
  }
}"#;

    fn body(raw: &str) -> Value {
        serde_json::from_str(raw).expect("captured JSON")
    }

    #[test]
    fn sheets_and_slides_join_the_probe_on_an_id_that_cannot_exist() {
        for service in ["sheets", "slides"] {
            let e = entry(service);
            assert_eq!(e.healthy, Healthy::NotFound);
            assert!(e.probe_url.contains(absent_id!()), "{}", e.probe_url);
        }
        let services: Vec<&str> = GOOGLE_API_LIBRARY.iter().map(|e| e.service).collect();
        assert_eq!(services, ["drive", "calendar", "gmail", "sheets", "slides"]);
    }

    #[test]
    fn service_disabled_reads_disabled_in_googles_own_words_with_its_activation_page() {
        for (service, raw) in [("sheets", SHEETS_DISABLED), ("slides", SLIDES_DISABLED)] {
            let b = body(raw);
            let got = classify(entry(service), 403, &b);
            assert_eq!(got.state, "disabled", "{service}");
            let message = b["error"]["message"].as_str().unwrap();
            assert_eq!(got.detail.as_deref(), Some(message), "quoted whole");
            assert_eq!(
                got.console_url,
                format!(
                    "https://console.developers.google.com/apis/api/{service}.googleapis.com/overview?project=00000000000"
                )
            );
        }
    }

    #[test]
    fn a_404_is_health_only_for_a_probe_that_asks_for_an_absent_id() {
        let b = body(DRIVE_NOT_FOUND);
        let sheets = classify(entry("sheets"), 404, &b);
        assert_eq!(sheets.state, "ok");
        assert_eq!(sheets.detail, None);
        assert_eq!(
            sheets.console_url,
            "https://console.cloud.google.com/apis/library/sheets.googleapis.com"
        );
        // The same answer to a probe that expects a real read is a failure,
        // and Google's sentence is the detail.
        let drive = classify(entry("drive"), 404, &b);
        assert_eq!(drive.state, "error");
        assert_eq!(
            drive.detail.as_deref(),
            Some("File not found: talaria-api-health-probe-nonexistent.")
        );
    }

    #[test]
    fn success_is_ok_and_a_bodyless_failure_names_its_status() {
        assert_eq!(classify(entry("gmail"), 200, &Value::Null).state, "ok");
        let got = classify(entry("slides"), 500, &Value::Null);
        assert_eq!(got.state, "error");
        assert_eq!(got.detail.as_deref(), Some("Google answered 500."));
        let got = classify(entry("calendar"), 401, &Value::Null);
        assert_eq!(
            got.detail.as_deref(),
            Some("Google rejected the access token. Reconnect the org account.")
        );
    }

    #[test]
    fn a_rejected_token_says_reconnect_and_quotes_google() {
        let got = classify(entry("sheets"), 401, &body(SHEETS_BAD_TOKEN));
        assert_eq!(got.state, "error");
        assert_eq!(
            got.detail.as_deref(),
            Some(
                "Google rejected the access token. Reconnect the org account. Google said: Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project."
            )
        );
    }

    #[test]
    fn google_signals_disabled_in_all_three_dialects() {
        let details = json!({ "error": { "details": [{ "reason": "SERVICE_DISABLED" }] } });
        let legacy = json!({ "error": { "errors": [{ "reason": "accessNotConfigured" }] } });
        let sentence = json!({ "error": { "message": "Google Calendar API has not been used in project 12 before or it is disabled" } });
        assert!(is_disabled(403, &details));
        assert!(is_disabled(404, &legacy));
        assert!(is_disabled(403, &sentence));
        // Only on 403/404, and only when the error object says so.
        assert!(!is_disabled(500, &details));
        assert!(!is_disabled(403, &json!({})));
        assert!(!is_disabled(
            403,
            &json!({ "error": { "message": "quota exceeded" } })
        ));
        assert!(!is_disabled(404, &body(DRIVE_NOT_FOUND)));
    }

    #[test]
    fn only_a_google_console_page_is_taken_as_the_activation_link() {
        let foreign = json!({ "error": { "details": [{ "metadata": { "activationUrl": "https://example.com/enable" } }] } });
        assert_eq!(activation_url(&foreign), None);
        assert!(activation_url(&body(SHEETS_DISABLED)).is_some());
    }

    #[test]
    fn the_detail_is_googles_message_capped_never_reworded() {
        let long = "x".repeat(DETAIL_CAP + 100);
        let v = json!({ "error": { "message": long } });
        assert_eq!(
            google_message(&v).map(|m| m.chars().count()),
            Some(DETAIL_CAP)
        );
        assert_eq!(
            google_message(&json!({ "error": { "message": "  " } })),
            None
        );
    }
}

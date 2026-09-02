// google/api-health.ts — is the Google side actually usable? OAuth consent
// succeeds even when the project's APIs are disabled — Google only checks at
// call time — so a freshly connected org account still 403s on the first Drive
// export / Calendar read / Gmail fetch until someone flips the three library
// switches in Google Cloud Console. This module makes that checkable on demand
// from the Admin UI instead of discovering it one broken surface at a time.
//
// GOOGLE_API_LIBRARY itself lives client-side (@/lib/google-apis — the Admin
// UI renders it as setup instructions); the server half is mirrored here
// because @/server must not reach a browser bundle in EITHER language.

use serde_json::Value;
use sqlx::PgPool;

use crate::gateway::provider::http;
use crate::google::connections::TokenError;
use crate::google::org::get_org_access_token;
use crate::secretbox::SecretBox;

/// One of the three services the library table names — drive, calendar, gmail.
struct LibraryEntry {
    service: &'static str,
    name: &'static str,
    console_url: &'static str,
    probe_url: &'static str,
}

/// The library (lib/google-apis.ts GOOGLE_API_LIBRARY) joined with each
/// service's probe — one cheapest-possible authenticated call per service,
/// chosen to stay inside the scopes WORKSPACE_SCOPES already grants (no extra
/// consent to test).
const GOOGLE_API_LIBRARY: [LibraryEntry; 3] = [
    LibraryEntry {
        service: "drive",
        name: "Google Drive API",
        console_url: "https://console.cloud.google.com/apis/library/drive.googleapis.com",
        // One file of the root listing — drive.readonly grants it, and unlike
        // about.get it has no required params to get wrong.
        probe_url: "https://www.googleapis.com/drive/v3/files?pageSize=1&fields=files(id)",
    },
    LibraryEntry {
        service: "calendar",
        name: "Google Calendar API",
        console_url: "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com",
        // One event of the primary calendar — calendar.events grants exactly
        // this.
        probe_url: "https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults=1&fields=nextPageToken",
    },
    LibraryEntry {
        service: "gmail",
        name: "Gmail API",
        console_url: "https://console.cloud.google.com/apis/library/gmail.googleapis.com",
        // The Gmail profile (mailbox address + history id) — gmail.modify
        // covers it.
        probe_url: "https://www.googleapis.com/gmail/v1/users/me/profile",
    },
];

/// One probe's answer (GoogleApiHealth) — wire order pinned (service, name,
/// consoleUrl, state, detail).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleApiHealth {
    pub service: &'static str,
    pub name: &'static str,
    pub console_url: &'static str,
    /// ok · disabled (enable it in the console) · error (anything else)
    pub state: &'static str,
    /// Short human sentence for the failing states; null when ok.
    pub detail: Option<String>,
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

/// Probe one service with an already-resolved org token. A transport-level
/// failure is an `Err` — TS's fetch would reject straight out of
/// probeOrgGoogleApis and fail the whole health call, not report one service
/// as red.
async fn probe(entry: &LibraryEntry, token: &str) -> Result<GoogleApiHealth, String> {
    let res = http()
        .get(entry.probe_url)
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("google health probe request: {e}"))?;
    if res.status().is_success() {
        return Ok(GoogleApiHealth {
            service: entry.service,
            name: entry.name,
            console_url: entry.console_url,
            state: "ok",
            detail: None,
        });
    }
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    if is_disabled(status, &body) {
        return Ok(GoogleApiHealth {
            service: entry.service,
            name: entry.name,
            console_url: entry.console_url,
            state: "disabled",
            detail: Some("Not enabled in this Google Cloud project.".into()),
        });
    }
    if status == 401 {
        return Ok(GoogleApiHealth {
            service: entry.service,
            name: entry.name,
            console_url: entry.console_url,
            state: "error",
            detail: Some("Google rejected the access token. Reconnect the org account.".into()),
        });
    }
    Ok(GoogleApiHealth {
        service: entry.service,
        name: entry.name,
        console_url: entry.console_url,
        state: "error",
        detail: Some(
            body.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(|m| m.chars().take(200).collect())
                .unwrap_or_else(|| format!("Google answered {status}.")),
        ),
    })
}

/// Probe all three services with the org connection's access token. An
/// `Err(NotConnected)` is the dead-org-connection throw (GoogleNotConnected) —
/// the route maps it to a reconnect prompt; `Err(Failed)` is the token read
/// itself blowing up (InvalidGrant etc.).
pub async fn probe_org_google_apis(
    pg: &PgPool,
    sb: &SecretBox,
    now_ms: i64,
) -> Result<Vec<GoogleApiHealth>, crate::google::errors::GoogleError> {
    let token = get_org_access_token(pg, sb, now_ms)
        .await
        .map_err(|e: TokenError| crate::google::errors::GoogleError::Failed(e.to_string()))?;
    let Some(token) = token else {
        return Err(crate::google::errors::GoogleError::NotConnected);
    };
    // TS's Promise.all over the library — three sequential probes cost three
    // Google round-trips (+300ms) for nothing; join_all keeps the library's
    // order and surfaces the first failure, as Promise.all does.
    let out: Vec<GoogleApiHealth> =
        futures_util::future::join_all(GOOGLE_API_LIBRARY.iter().map(|entry| probe(entry, &token)))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, String>>()
            .map_err(crate::google::errors::GoogleError::Failed)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    }

    #[test]
    fn the_detail_sentence_truncates_at_200_chars() {
        let long = "x".repeat(300);
        let v = json!({ "error": { "message": long } });
        let m: String = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .map(|m| m.chars().take(200).collect())
            .unwrap();
        assert_eq!(m.len(), 200);
    }
}

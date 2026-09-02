// google/provisioning.ts — org workspace provisioning: the one-click "make
// the shared Google workspace real" pass. Two containers, both created AS the
// connected org account and shared with its Workspace domain so every person
// there can reach them without anyone hand-configuring sharing in Google:
//
//   · an org calendar ("Talaria") — domain-wide WRITER, so members see and
//     edit it; agents already read/write whatever calendar_id points at
//   · a Shared Drive ("Talaria") — domain-wide FILE ORGANIZER (a content
//     manager: add, edit, move, trash), with artifact exports repointed at
//     its root so team files are born team-owned
//
// Every step is idempotent: a stored id is verified against Google before it
// is trusted (a calendar deleted out from under us is recreated, not 404ed on
// forever), and share rules are checked-then-added so re-running the pass
// never duplicates. Outcomes come back per-item — one failed container must
// not hide the other's success — and the route hands them to the Admin UI.

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::gateway::provider::http;
use crate::google::connections::TokenError;
use crate::google::oauth::{email_domain_of, encode_uri_component};
use crate::google::org::{
    OrgTargets, OrgTargetsPatch, get_org_access_token, get_org_connection_status,
    set_org_shared_drive, set_org_targets,
};
use crate::secretbox::SecretBox;

const CAL_BASE: &str = "https://www.googleapis.com/calendar/v3";
const DRIVE_BASE: &str = "https://www.googleapis.com/drive/v3";

pub const ORG_CALENDAR_NAME: &str = "Talaria";
pub const ORG_DRIVE_NAME: &str = "Talaria";

// Full scopes, not the per-file/per-event ones — see ORG_CONNECT_SCOPES in
// google_oauth.rs for why provisioning needs the wider grants.
const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar";
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";

/// One container's outcome (ProvisionResult) — the TS tagged shape, field
/// order pinned: `{ok, state, id}` or `{ok, error, message}`.
pub enum ProvisionResult {
    Ok {
        state: &'static str,
        id: String,
    },
    Fail {
        error: &'static str,
        message: String,
    },
}

impl serde::Serialize for ProvisionResult {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            ProvisionResult::Ok { state, id } => {
                let mut s = serializer.serialize_struct("ProvisionResult", 3)?;
                s.serialize_field("ok", &true)?;
                s.serialize_field("state", state)?;
                s.serialize_field("id", id)?;
                s.end()
            }
            ProvisionResult::Fail { error, message } => {
                let mut s = serializer.serialize_struct("ProvisionResult", 3)?;
                s.serialize_field("ok", &false)?;
                s.serialize_field("error", error)?;
                s.serialize_field("message", message)?;
                s.end()
            }
        }
    }
}

/// A failed Google call, carrying the HTTP status so `classify` can read it.
struct GoogleCallError {
    message: String,
    status: u16,
}

/// What the try-block can die of: a Google call (classified) or anything else
/// (a targets write, a transport failure) — TS's catch treats both as
/// `failed(surface, err)`.
enum CallFail {
    Google(GoogleCallError),
    Other(String),
}

impl From<GoogleCallError> for CallFail {
    fn from(e: GoogleCallError) -> Self {
        CallFail::Google(e)
    }
}

impl From<String> for CallFail {
    fn from(m: String) -> Self {
        CallFail::Other(m)
    }
}

/// A Google error message, or a stand-in for a body with none.
async fn err_text(res: reqwest::Response) -> String {
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    body.get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| format!("Google answered {status}."))
}

/// The Google error classes this pass distinguishes (classify). Scope/API
/// messages become reconnect prompts even past the stored-scope preflight
/// (Google can still refuse a granted scope per-admin); 403s mentioning
/// shared drives mark the Shared-Drives-need-Workspace wall.
fn classify(err: &GoogleCallError) -> &'static str {
    let text = format!("{} {}", err.message, err.status).to_lowercase();
    if [
        "insufficient",
        "access_token_scope",
        "accessnotconfigured",
        "service_disabled",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        return "reconnect_needed";
    }
    // /shared.?drives?|team.?drives?/i — the optional gap catches "shared
    // drive", "shared drives", and the hyphenated spellings in one pattern.
    if err.status == 403 {
        let re = regex::Regex::new("(?i)shared.?drives?|team.?drives?").expect("static regex");
        if re.is_match(&text) {
            return "consumer_account";
        }
    }
    "google_error"
}

/// preflight's answer: the pieces both provisions need, or the failure that
/// says why not. `Err` is a THROW (a dead token read) — it escapes to
/// provisionWorkspace's catch, not the outcome.
enum Preflight {
    Ready {
        token: String,
        domain: String,
        targets: OrgTargets,
    },
    Fail(ProvisionResult),
}

/// Connected org account + the scope a container needs, or the outcome saying
/// why not. Checking the STORED scope first turns Google's opaque 403 into
/// "reconnect" — the fix an admin can actually act on.
async fn preflight(
    pg: &PgPool,
    sb: &SecretBox,
    scope: &str,
    surface: &str,
    now_ms: i64,
) -> Result<Preflight, String> {
    let status = get_org_connection_status(pg)
        .await
        .map_err(|e| e.to_string())?;
    if !status.connected {
        return Ok(Preflight::Fail(ProvisionResult::Fail {
            error: "not_connected",
            message: "Connect the org Google account first.".into(),
        }));
    }
    if !status.scope.iter().any(|s| s == scope) {
        return Ok(Preflight::Fail(ProvisionResult::Fail {
            error: "reconnect_needed",
            message: format!(
                "Reconnect the org account to grant {surface} provisioning (one-time, for the wider scopes)."
            ),
        }));
    }
    let Some(domain) = email_domain_of(status.email.as_deref()) else {
        return Ok(Preflight::Fail(ProvisionResult::Fail {
            error: "no_domain",
            message: "The org account has no usable email address to share with.".into(),
        }));
    };
    let token = get_org_access_token(pg, sb, now_ms)
        .await
        .map_err(|e: TokenError| e.to_string())?;
    let Some(token) = token else {
        return Ok(Preflight::Fail(ProvisionResult::Fail {
            error: "not_connected",
            message: "Connect the org Google account first.".into(),
        }));
    };
    Ok(Preflight::Ready {
        token,
        domain,
        targets: OrgTargets {
            drive_folder_id: status.targets.drive_folder_id,
            calendar_id: status.targets.calendar_id,
            send_as: status.targets.send_as,
            shared_drive_id: status.targets.shared_drive_id,
        },
    })
}

// ── Calendar ─────────────────────────────────────────────────────────────────

/// A calendar id that still exists. A calendar deleted in Google (an admin
/// cleaning up) must be recreated, not trusted forever.
async fn calendar_exists(token: &str, id: &str) -> Result<bool, String> {
    let res = http()
        .get(format!(
            "{CAL_BASE}/calendars/{}?fields=id",
            encode_uri_component(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("calendar exists request: {e}"))?;
    Ok(res.status().is_success())
}

/// True when a domain-wide rule for `domain` is already on the calendar.
async fn calendar_shared_with_domain(token: &str, id: &str, domain: &str) -> Result<bool, String> {
    let res = http()
        .get(format!(
            "{CAL_BASE}/calendars/{}/acl?fields=items(scope)",
            encode_uri_component(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("calendar acl read request: {e}"))?;
    if !res.status().is_success() {
        // Unreadable → let the insert run; duplicates are tolerated below.
        return Ok(false);
    }
    let data: Value = res
        .json()
        .await
        .map_err(|e| format!("calendar acl read body: {e}"))?;
    Ok(data
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|i| {
                let scope = i.get("scope");
                scope
                    .and_then(|s| s.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|t| t == "domain")
                    && scope
                        .and_then(|s| s.get("value"))
                        .and_then(Value::as_str)
                        .is_some_and(|v| v.to_lowercase() == domain)
            })
        }))
}

async fn share_calendar_with_domain(token: &str, id: &str, domain: &str) -> Result<(), CallFail> {
    let res = http()
        .post(format!(
            "{CAL_BASE}/calendars/{}/acl",
            encode_uri_component(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({ "scope": { "type": "domain", "value": domain }, "role": "writer" })
                .to_string(),
        )
        .send()
        .await
        .map_err(|e| CallFail::Other(format!("calendar acl write request: {e}")))?;
    if !res.status().is_success() && res.status().as_u16() != 409 {
        let status = res.status().as_u16();
        return Err(GoogleCallError {
            message: err_text(res).await,
            status,
        }
        .into());
    }
    // Already shared (a race, or a recreate of a calendar Google still
    // remembers) adds up to the success it was aiming at, not an error.
    Ok(())
}

/// Create (or re-use) the org calendar and share it with the Workspace
/// domain. `Err` is a THROW (preflight's dead token read) — the caller maps
/// it through thrown_outcome.
pub async fn provision_org_calendar(
    pg: &PgPool,
    sb: &SecretBox,
    now_ms: i64,
) -> Result<ProvisionResult, String> {
    let (token, domain, targets) =
        match preflight(pg, sb, CALENDAR_SCOPE, "calendar", now_ms).await? {
            Preflight::Fail(fail) => return Ok(fail),
            Preflight::Ready {
                token,
                domain,
                targets,
            } => (token, domain, targets),
        };
    let run = async {
        let mut id = targets.calendar_id.clone();
        let mut state: &'static str;
        if let Some(existing) = id.as_deref()
            && calendar_exists(&token, existing).await?
        {
            state = "reused";
        } else {
            let res = http()
                .post(format!("{CAL_BASE}/calendars"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(serde_json::json!({ "summary": ORG_CALENDAR_NAME }).to_string())
                .send()
                .await
                .map_err(|e| format!("calendar create request: {e}"))?;
            if !res.status().is_success() {
                let status = res.status().as_u16();
                return Err(GoogleCallError {
                    message: err_text(res).await,
                    status,
                }
                .into());
            }
            let created: Value = res
                .json()
                .await
                .map_err(|e| format!("calendar create body: {e}"))?;
            id = Some(
                created
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
            state = "created";
            set_org_targets(
                pg,
                &OrgTargetsPatch {
                    calendar_id: Some(id.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        let id = id.unwrap_or_default();

        if !calendar_shared_with_domain(&token, &id, &domain).await? {
            share_calendar_with_domain(&token, &id, &domain).await?;
            if state == "reused" {
                state = "shared";
            }
        }
        Ok::<ProvisionResult, CallFail>(ProvisionResult::Ok { state, id })
    };
    match run.await {
        Ok(result) => Ok(result),
        Err(fail) => Ok(failed("Calendar", fail)),
    }
}

// ── Shared Drive ─────────────────────────────────────────────────────────────

async fn shared_drive_exists(token: &str, id: &str) -> Result<bool, String> {
    let res = http()
        .get(format!(
            "{DRIVE_BASE}/drives/{}?supportsAllDrives=true&fields=id",
            encode_uri_component(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("drive read request: {e}"))?;
    Ok(res.status().is_success())
}

/// True when a domain-wide permission is already on the drive.
async fn drive_shared_with_domain(token: &str, id: &str) -> Result<bool, String> {
    let res = http()
        .get(format!(
            "{DRIVE_BASE}/drives/{}/permissions?supportsAllDrives=true&fields=permissions(type)",
            encode_uri_component(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("drive permissions read request: {e}"))?;
    if !res.status().is_success() {
        return Ok(false);
    }
    let data: Value = res
        .json()
        .await
        .map_err(|e| format!("drive permissions read body: {e}"))?;
    Ok(data
        .get("permissions")
        .and_then(Value::as_array)
        .is_some_and(|perms| {
            perms
                .iter()
                .any(|p| p.get("type").and_then(Value::as_str) == Some("domain"))
        }))
}

async fn share_drive_with_domain(token: &str, id: &str) -> Result<(), CallFail> {
    let res = http()
        .post(format!(
            "{DRIVE_BASE}/drives/{}/permissions?supportsAllDrives=true",
            encode_uri_component(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        // fileOrganizer = the shared drive's "Content manager": add, edit,
        // move, trash. The team's members manage the team's files — writer
        // would fence them out of the organizing half of "everyone can access
        // it".
        .body(
            serde_json::json!({ "role": "fileOrganizer", "type": "domain", "allowFileDiscovery": true })
                .to_string(),
        )
        .send()
        .await
        .map_err(|e| CallFail::Other(format!("drive permissions write request: {e}")))?;
    if !res.status().is_success() && res.status().as_u16() != 409 {
        let status = res.status().as_u16();
        return Err(GoogleCallError {
            message: err_text(res).await,
            status,
        }
        .into());
    }
    Ok(())
}

/// Create (or re-use) the Shared Drive and grant the Workspace domain access.
/// Exports are repointed at the drive root — team files, team-owned.
pub async fn provision_shared_drive(
    pg: &PgPool,
    sb: &SecretBox,
    now_ms: i64,
) -> Result<ProvisionResult, String> {
    let (token, targets) = match preflight(pg, sb, DRIVE_SCOPE, "Drive", now_ms).await? {
        Preflight::Fail(fail) => return Ok(fail),
        Preflight::Ready { token, targets, .. } => (token, targets),
    };
    let run = async {
        let mut id = targets.shared_drive_id.clone();
        let mut state: &'static str;
        if let Some(existing) = id.as_deref()
            && shared_drive_exists(&token, existing).await?
        {
            state = "reused";
        } else {
            let res = http()
                .post(format!("{DRIVE_BASE}/drives?requestId={}", Uuid::new_v4()))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(serde_json::json!({ "name": ORG_DRIVE_NAME }).to_string())
                .send()
                .await
                .map_err(|e| format!("drive create request: {e}"))?;
            if !res.status().is_success() {
                let status = res.status().as_u16();
                return Err(GoogleCallError {
                    message: err_text(res).await,
                    status,
                }
                .into());
            }
            let created: Value = res
                .json()
                .await
                .map_err(|e| format!("drive create body: {e}"))?;
            id = Some(
                created
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
            state = "created";
            set_org_shared_drive(pg, id.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            // The drive is the point — files land in it unless an admin
            // retargets.
            set_org_targets(
                pg,
                &OrgTargetsPatch {
                    drive_folder_id: Some(id.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        let id = id.unwrap_or_default();

        if !drive_shared_with_domain(&token, &id).await? {
            share_drive_with_domain(&token, &id).await?;
            if state == "reused" {
                state = "shared";
            }
        }
        Ok::<ProvisionResult, CallFail>(ProvisionResult::Ok { state, id })
    };
    match run.await {
        Ok(result) => Ok(result),
        Err(fail) => Ok(failed("Shared Drive", fail)),
    }
}

/// A thrown Google call → its outcome (failed). The consumer-account wall gets
/// the sentence an admin can act on; everything else keeps Google's words.
fn failed(surface: &str, fail: CallFail) -> ProvisionResult {
    let (kind, message) = match fail {
        CallFail::Google(gerr) => {
            let kind = classify(&gerr);
            let message = if kind == "consumer_account" {
                "Shared Drives need a Google Workspace account — the connected org account is a consumer account."
                    .to_string()
            } else {
                format!(
                    "{surface} provisioning failed: {}",
                    gerr.message.chars().take(300).collect::<String>()
                )
            };
            (kind, message)
        }
        CallFail::Other(m) => (
            "google_error",
            format!(
                "{surface} provisioning failed: {}",
                m.chars().take(300).collect::<String>()
            ),
        ),
    };
    ProvisionResult::Fail {
        error: kind,
        message,
    }
}

/// Run the requested provisions. Each runs and reports independently; a throw
/// from one (a dead refresh token) becomes its failure outcome, not a lost
/// sibling result.
pub async fn provision_workspace(
    pg: &PgPool,
    sb: &SecretBox,
    input: ProvisionRequest,
    now_ms: i64,
) -> Result<ProvisionWorkspaceResult, String> {
    let mut out = ProvisionWorkspaceResult::default();
    if input.calendar {
        out.calendar = Some(match provision_org_calendar(pg, sb, now_ms).await {
            Ok(result) => result,
            Err(e) => thrown_outcome(&e),
        });
    }
    if input.drive {
        out.drive = Some(match provision_shared_drive(pg, sb, now_ms).await {
            Ok(result) => result,
            Err(e) => thrown_outcome(&e),
        });
    }
    Ok(out)
}

/// The request body's two flags.
pub struct ProvisionRequest {
    pub calendar: bool,
    pub drive: bool,
}

/// `{calendar?, drive?}` — each present only when requested.
#[derive(Default, serde::Serialize)]
pub struct ProvisionWorkspaceResult {
    pub calendar: Option<ProvisionResult>,
    pub drive: Option<ProvisionResult>,
}

/// thrownOutcome — a preflight throw (the only throws left) becomes its
/// failure outcome. `not_connected` for the GoogleNotConnected-shaped one,
/// Google's words for everything else.
fn thrown_outcome(err: &str) -> ProvisionResult {
    if err == "not_connected" {
        return ProvisionResult::Fail {
            error: "not_connected",
            message: "Connect the org Google account first.".into(),
        };
    }
    ProvisionResult::Fail {
        error: "google_error",
        message: err.chars().take(300).collect(),
    }
}

/// Scope readiness for the Admin panel (provisioningReadiness): can this
/// connection create each container, or does it need the one-time
/// wider-scope reconnect? Wire order pinned, camelCase.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningReadiness {
    pub connected: bool,
    pub email: Option<String>,
    pub calendar_scope: bool,
    pub drive_scope: bool,
}

pub async fn provisioning_readiness(pg: &PgPool) -> Result<ProvisioningReadiness, String> {
    let status = get_org_connection_status(pg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ProvisioningReadiness {
        connected: status.connected,
        email: status.email,
        calendar_scope: status.scope.iter().any(|s| s == CALENDAR_SCOPE),
        drive_scope: status.scope.iter().any(|s| s == DRIVE_SCOPE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reads_all_four_reconnect_dialects() {
        let mk = |message: &str, status: u16| GoogleCallError {
            message: message.into(),
            status,
        };
        assert_eq!(
            classify(&mk("Request had insufficient authentication scopes", 403)),
            "reconnect_needed"
        );
        assert_eq!(classify(&mk("ACCESS_TOKEN_SCOPE", 403)), "reconnect_needed");
        assert_eq!(
            classify(&mk("accessNotConfigured: Project not configured", 403)),
            "reconnect_needed"
        );
        assert_eq!(classify(&mk("SERVICE_DISABLED", 404)), "reconnect_needed");
    }

    #[test]
    fn classify_marks_the_shared_drive_wall_only_on_403() {
        let mk = |message: &str, status: u16| GoogleCallError {
            message: message.into(),
            status,
        };
        assert_eq!(
            classify(&mk("Shared drives are only available to Workspace", 403)),
            "consumer_account"
        );
        // Team drive, plural, and the hyphenated spelling all match.
        assert_eq!(
            classify(&mk("team-drive not supported", 403)),
            "consumer_account"
        );
        assert_eq!(classify(&mk("Shared Drives", 403)), "consumer_account");
        // Same words, wrong status → google_error.
        assert_eq!(
            classify(&mk("Shared drives are only available to Workspace", 400)),
            "google_error"
        );
        assert_eq!(classify(&mk("boom", 500)), "google_error");
    }

    #[test]
    fn failed_messages_keep_the_consumer_sentence_and_truncate() {
        let wall = failed(
            "Shared Drive",
            CallFail::Google(GoogleCallError {
                message: "shared drives need workspace".into(),
                status: 403,
            }),
        );
        match wall {
            ProvisionResult::Fail { error, message } => {
                assert_eq!(error, "consumer_account");
                assert!(message.starts_with("Shared Drives need a Google Workspace account"));
            }
            _ => panic!("expected a failure"),
        }
        let long = failed("Calendar", CallFail::Other("x".repeat(400)));
        match long {
            ProvisionResult::Fail { error, message } => {
                assert_eq!(error, "google_error");
                assert_eq!(
                    message,
                    format!("Calendar provisioning failed: {}", "x".repeat(300))
                );
            }
            _ => panic!("expected a failure"),
        }
    }

    #[test]
    fn the_outcome_serializes_in_the_ts_field_order() {
        let ok = ProvisionResult::Ok {
            state: "created",
            id: "cal-1".into(),
        };
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"ok":true,"state":"created","id":"cal-1"}"#
        );
        let fail = ProvisionResult::Fail {
            error: "not_connected",
            message: "Connect the org Google account first.".into(),
        };
        assert_eq!(
            serde_json::to_string(&fail).unwrap(),
            r#"{"ok":false,"error":"not_connected","message":"Connect the org Google account first."}"#
        );
    }

    #[test]
    fn thrown_outcomes_map_not_connected_and_everything_else() {
        match thrown_outcome("not_connected") {
            ProvisionResult::Fail { error, message } => {
                assert_eq!(error, "not_connected");
                assert_eq!(message, "Connect the org Google account first.");
            }
            _ => panic!("expected a failure"),
        }
        match thrown_outcome("google token refresh failed: 400 invalid_grant") {
            ProvisionResult::Fail { error, message } => {
                assert_eq!(error, "google_error");
                assert_eq!(message, "google token refresh failed: 400 invalid_grant");
            }
            _ => panic!("expected a failure"),
        }
    }
}

// GET /api/integrations/google/org/callback — store the SHARED org
// connection. The shared connect body plus the org's two differences: only an
// admin may tie the org's containers to a Google account, and the landing
// page is the admin panel whose googleOrg flash reads the status param.
// A connect that landed is audited: this is the widest Google credential in
// the system, and the row's `connected_by` alone does not say when it moved.

use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use serde_json::json;

use talaria_api_facades::google::oauth::{ConnectFlavor, handle_connect_callback};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::percent_encode;
use talaria_session::actor_of;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let (response, connected) = handle_connect_callback(
        &state,
        &headers,
        &uri,
        ConnectFlavor::Org,
        "integrations/google/org",
        |status| format!("/admin?googleOrg={}", percent_encode(status)),
    )
    .await;
    if let Some(account) = connected {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&account.user),
                action: "google.org_connect",
                target_type: "google_org_connection",
                target_id: None,
                target_label: account.email.as_deref(),
                before: None,
                after: Some(json!({ "email": account.email })),
            },
        )
        .await;
    }
    response
}

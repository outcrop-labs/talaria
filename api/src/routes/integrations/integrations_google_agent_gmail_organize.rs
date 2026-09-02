// POST /api/integrations/google/agent/gmail/organize — file/archive/read
// messages by label. THE HITL LINE, stated once because it is the one
// judgment this route makes: sends and invites leave the building under the
// owner's identity and queue for approval; filing, archiving and mark-read
// stay INSIDE the mailbox and are reversible, so they apply immediately — that
// immediacy is the feature ("clean up my inbox" that takes fifty approval
// cards is not cleanup). Destructive labels (TRASH/SPAM) are refused in the
// service layer: nothing this route can do removes mail from All Mail, which
// is what keeps the immediacy honest.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::{AgentSubject, refuse_legacy, require_agent};
use crate::body::{as_object, optional_string_array_member, parse, string_array_member};
use crate::error::{house_error, house_error_msg};
use crate::google::agent::resolve_agent_google;
use crate::google::errors::{GoogleError, google_fail};
use crate::google::gmail::{OrganizeInput, organize_emails_with_token};
use crate::state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // Mutating the owner's mailbox — a legacy shared-key caller only ASSERTS
    // which agent it is, so it never reaches the token.
    if let Some(denied) = refuse_legacy(&caller, "Gmail access") {
        return denied;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "No Google account is connected for this agent (its owner, or the org account).",
        );
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // ids is required: at least one, each 1–128 chars, 1–100 of them.
    let ids = match string_array_member(obj, "ids", 1, 128, 1, 100) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let add_labels = match optional_string_array_member(obj, "addLabels", 1, 120, 10) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let remove_labels = match optional_string_array_member(obj, "removeLabels", 1, 120, 10) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let input = OrganizeInput {
        ids: &ids,
        add_labels: add_labels.as_deref().unwrap_or(&[]),
        remove_labels: remove_labels.as_deref().unwrap_or(&[]),
    };
    match organize_emails_with_token(&google.token, &input).await {
        Ok(updated) => Json(json!({
            "updated": updated,
            "note": "filed — labels applied and archived mail stays in All Mail; nothing was deleted or sent",
        }))
        .into_response(),
        // An unknown label is a routing mistake by the caller, not a Google
        // outage: answer it as a 400 the agent can act on (create_label), not
        // a 502 it can only relay.
        Err(GoogleError::Failed(m)) if m.starts_with("gmail organize: ") => {
            house_error_msg(StatusCode::BAD_REQUEST, "bad request", &m)
        }
        Err(e) => google_fail(e, "Gmail"),
    }
}

/// Epoch-ms clock — the one time the agent mailbox reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// /api/integrations/google/agent/gmail — port of
// ui/src/routes/api/integrations/google.agent.gmail.ts. Agent-facing Gmail: a
// personal assistant acts as its owner; a general fleet agent acts on the
// shared ORG mailbox.
// GET  → read recent mail (free)
// POST → DRAFT an email; queued for approval (the owner, or an admin for org).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::agent_auth::{AgentSubject, refuse_legacy, require_agent};
use crate::body::{as_object, optional_max_string_member, parse, string_member};
use crate::error::{house_error, house_error_msg, thrown_internal_error};
use crate::google::agent::{resolve_agent_google, resolve_agent_principal};
use crate::google::errors::google_fail;
use crate::google::gmail::list_recent_messages_with_token;
use crate::google::oauth::query_pairs;
use crate::google::pending_actions::{QueueAction, queue_action};
use crate::realtime::RealtimeDeps;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // Acting as a HUMAN — the owner's mailbox (or the shared org one). A
    // legacy shared-key caller only ASSERTS which agent it is, so it never
    // reaches a token; the refusal names the container to roll.
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
    // `|| 'in:inbox'` — absent OR empty folds to the inbox default.
    let q = query_pairs(uri.query())
        .get("q")
        .cloned()
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| "in:inbox".to_string());
    match list_recent_messages_with_token(&google.token, 8, &q).await {
        Ok(messages) => Json(json!({ "messages": messages })).into_response(),
        Err(e) => google_fail(e, "Gmail"),
    }
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    if let Some(denied) = refuse_legacy(&caller, "Gmail access") {
        return denied;
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let to = match string_member(obj, "to", 3, 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // `.default('')` — absent folds to the empty string.
    let subject = match optional_max_string_member(obj, "subject", 500) {
        Ok(Some(s)) => s,
        Ok(None) => String::new(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let text = match optional_max_string_member(obj, "body", 50_000) {
        Ok(Some(s)) => s,
        Ok(None) => String::new(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let cc = match optional_max_string_member(obj, "cc", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let bcc = match optional_max_string_member(obj, "bcc", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[integrations/google/agent] principal read failed: {e}");
            return thrown_internal_error();
        }
    };
    // The payload IS the validated draft, stored as drafted and executed as
    // stored at approve time; subject/body always ride (their defaults),
    // cc/bcc only when the request carried them.
    let mut payload = serde_json::Map::new();
    payload.insert("to".into(), json!(to));
    payload.insert("subject".into(), json!(subject));
    payload.insert("body".into(), json!(text));
    if let Some(c) = &cc {
        payload.insert("cc".into(), json!(c));
    }
    if let Some(b) = &bcc {
        payload.insert("bcc".into(), json!(b));
    }
    let draft_summary = format!(
        "Email to {to}: {}",
        if subject.is_empty() {
            "(no subject)"
        } else {
            subject.as_str()
        }
    );
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let action = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind: "gmail_send",
            summary: &draft_summary,
            payload: &Value::Object(payload),
            agent_model: &agent_model,
            owner_user_id: principal.owner_user_id.as_deref(),
            is_org: principal.is_org,
        },
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[integrations/google/agent] queue failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "pending": { "id": action.id, "status": "pending" },
        "message": if principal.is_org {
            "Drafted — waiting for an admin to approve before it sends."
        } else {
            "Drafted — waiting for the owner to approve before it sends."
        },
    }))
    .into_response()
}

/// Date.now() — the one clock the agent mailbox reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

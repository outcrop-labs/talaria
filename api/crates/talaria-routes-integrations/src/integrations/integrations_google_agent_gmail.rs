// /api/integrations/google/agent/gmail. Agent-facing Gmail: a personal
// assistant acts as its owner; a general fleet agent acts on the shared ORG
// mailbox.
// GET  → read recent mail (free)
// POST → DRAFT an email; queued for approval (the owner, or an admin for org).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::gmail::list_recent_messages_with_token;
use talaria_api_facades::google::oauth::query_pairs;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    // Acting as a HUMAN — the owner's mailbox (or the shared org one). A
    // legacy shared-key caller only ASSERTS which agent it is, so it never
    // reaches a token; the refusal names the container to roll.
    if let Some(denied) = refuse_legacy(&caller, "Gmail access") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "No Google account is connected for this agent (its owner, or the org account).",
        ));
    };
    // Absent OR empty q folds to the inbox default.
    let q = query_pairs(uri.query())
        .get("q")
        .cloned()
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| "in:inbox".to_string());
    Ok(
        match list_recent_messages_with_token(&google.token, 8, &q).await {
            Ok(messages) => Json(json!({ "messages": messages })).into_response(),
            Err(e) => google_fail(e, "Gmail"),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return Ok(gate),
    };
    if let Some(denied) = refuse_legacy(&caller, "Gmail access") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let to = match string_member(obj, "to", 3, 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Absent subject folds to the empty string.
    let subject = match optional_max_string_member(obj, "subject", 500) {
        Ok(Some(s)) => s,
        Ok(None) => String::new(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let text = match optional_max_string_member(obj, "body", 50_000) {
        Ok(Some(s)) => s,
        Ok(None) => String::new(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let cc = match optional_max_string_member(obj, "cc", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let bcc = match optional_max_string_member(obj, "bcc", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent] principal read failed",
                e,
            ));
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
    let queued = match queue_action(
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
        Ok(q) => q,
        Err(e) => return Ok(internal("[integrations/google/agent] queue failed", e)),
    };
    // A prior approval in this conversation unlocked draft_email. Execute
    // the draft now instead of asking again — the unlock chip already said so.
    if !principal.is_org
        && let Ok(Some(conv)) = talaria_chips::live_conversation_id(&state.pg, &agent_model).await
        && talaria_chips::tool_unlocked(&state.pg, &conv, "draft_email")
            .await
            .unwrap_or(false)
        && let Some(owner) = principal.owner_user_id.as_deref()
    {
        let sb = state.secretbox().await.unwrap_or_default();
        let _ = talaria_api_facades::google::pending_actions::decide_action(
            &state.pg,
            &sb,
            &queued.action.id,
            owner,
            false,
            "approve",
            talaria_agent_auth::now_ms(),
        )
        .await;
        return Ok(Json(json!({
            "pending": { "id": queued.action.id, "status": "executed" },
            "message": "Sent — this conversation already unlocked draft_email.",
        }))
        .into_response());
    }
    let message = if queued.already_pending {
        "An identical draft is already waiting for approval — nothing new queued."
    } else if principal.is_org {
        "Drafted — waiting for an admin to approve before it sends."
    } else {
        "Drafted — waiting for the owner to approve before it sends."
    };
    Ok(Json(json!({
        "pending": { "id": queued.action.id, "status": "pending" },
        "message": message,
    }))
    .into_response())
}

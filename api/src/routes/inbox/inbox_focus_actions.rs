// /api/inbox/focus/actions — port of ui/src/routes/api/inbox.focus.actions.ts.
// POST → execute a focus-inbox action: fire an action, confirm or cancel a
// pending decision, undo the last one. Every button on a queue card funnels
// through this one entry, under the Inbox lock; the timeline entry is
// attached before the lock drops so the response's entry agrees with the row
// it just wrote.
//
// The result's own `status` picks the HTTP code: stale → 409 (the item
// changed under the click), failed → 422 (the action refused), else 200. The
// lock-miss 409 is a DIFFERENT shape — {status, message}, the same envelope
// the card renders for a failed action — so the panel shows the sentence
// rather than an error toast.

use crate::body::{as_object, optional_string_member, optional_uuid_member, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::inbox_focus::conversation::{
    acquire_inbox_focus_lock, attach_timeline_to_action_result,
};
use crate::inbox_focus::{FocusActionInput, run_focus_action};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

fn validate(obj: &serde_json::Map<String, Value>) -> Result<FocusActionInput, String> {
    let key = optional_string_member(obj, "key", 600)?;
    let action_id = optional_string_member(obj, "actionId", 100)?;
    let payload = obj.get("payload").cloned();
    let command_decision_id = optional_uuid_member(obj, "commandDecisionId")?;
    let decision_id = optional_uuid_member(obj, "decisionId")?;
    // `z.string().min(20).max(200)` — tokens are ≥20 chars by construction;
    // a short one is a client bug, and the min is part of the contract.
    let confirmation_token = match obj.get("confirmationToken") {
        None => None,
        Some(_) => Some(string_member(obj, "confirmationToken", 20, 200)?),
    };
    let cancel_decision_id = optional_uuid_member(obj, "cancelDecisionId")?;
    let undo_decision_id = optional_uuid_member(obj, "undoDecisionId")?;
    Ok(FocusActionInput {
        key,
        action_id,
        payload,
        command_decision_id,
        decision_id,
        confirmation_token,
        cancel_decision_id,
        undo_decision_id,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let input = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let Some(_guard) = acquire_inbox_focus_lock(&user.id) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "failed",
                "message": "Your assistant is already handling another Inbox action.",
            })),
        )
            .into_response();
    };
    let result = match run_focus_action(&state, &user, &input).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("[inbox-focus] action failed: {e}");
            return thrown_internal_error();
        }
    };
    let result = match attach_timeline_to_action_result(&state, &user, result).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("[inbox-focus] timeline attach failed: {e}");
            return thrown_internal_error();
        }
    };
    let status = match result.get("status").and_then(Value::as_str) {
        Some("stale") => StatusCode::CONFLICT,
        Some("failed") => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::OK,
    };
    (status, Json(result)).into_response()
}

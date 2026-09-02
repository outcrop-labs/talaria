// /api/brief/reply — port of ui/src/routes/api/brief.reply.ts.
// POST { draftId, decision } → approve or reject a reply the assistant
// drafted: send it, or discard it.
//
// The one route in this feature that causes something to LEAVE — a message
// another person receives — so it is the one that refuses on staleness.
// Every authority check lives in `decide_draft`, scoped to the caller's own
// drafts, rather than here: a second copy of that rule beside the first is
// how the two come to disagree.

use crate::body::{as_object, enum_member, uuid_member};
use crate::daily_brief::delegation::{DraftOutcome, decide_draft};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The POST body, TS's `Body` zod shape.
struct ReplyBody {
    draft_id: String,
    decision: String,
}

fn validate(obj: &serde_json::Map<String, serde_json::Value>) -> Result<ReplyBody, String> {
    let draft_id = uuid_member(obj, "draftId")?;
    let decision = enum_member(obj, "decision", &["approve", "reject"])?;
    Ok(ReplyBody { draft_id, decision })
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
    let body = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let notify = crate::notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let outcome = match decide_draft(
        &notify,
        &user.id,
        &body.draft_id,
        body.decision == "approve",
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("[brief] draft decide failed: {e}");
            return thrown_internal_error();
        }
    };
    match outcome {
        DraftOutcome::Gone => {
            house_error(StatusCode::NOT_FOUND, "That draft is no longer available.")
        }
        // 409, not 400: the request was well-formed and was correct when the
        // person read it. The world moved underneath it, and the client's
        // answer is to ask for a fresh draft rather than to fix its input.
        DraftOutcome::Stale(message) => house_error(StatusCode::CONFLICT, message),
        DraftOutcome::Sent => Json(json!({ "status": "sent" })).into_response(),
        DraftOutcome::Rejected => Json(json!({ "status": "rejected" })).into_response(),
    }
}

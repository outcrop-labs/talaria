// POST /api/integrations/google/gmail/send — port of
// ui/src/routes/api/integrations/google.gmail.send.ts. Send a plain-text
// email as the user.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, optional_max_string_member, parse, string_member};
use crate::error::house_error;
use crate::google::errors::google_fail;
use crate::google::gmail::{SendInput, send_message};
use crate::session::require_user;
use crate::state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let to = match string_member(obj, "to", 3, 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // `.default('')` — absent folds to the empty string, never null.
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
    let input = SendInput {
        to: &to,
        subject: &subject,
        body: &text,
        cc: cc.as_deref(),
        bcc: bcc.as_deref(),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    match send_message(&state.pg, &sb, &user.id, now_ms(), &input).await {
        Ok((id, thread_id)) => {
            Json(json!({ "sent": { "id": id, "threadId": thread_id } })).into_response()
        }
        Err(e) => google_fail(e, "Gmail"),
    }
}

/// Date.now() — the one clock the send executes under.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

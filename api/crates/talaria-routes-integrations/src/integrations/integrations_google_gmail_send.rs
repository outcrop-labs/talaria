// POST /api/integrations/google/gmail/send — send a plain-text email as the
// user.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::now_ms;
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::gmail::{SendInput, send_message};
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let to = match string_member(obj, "to", 3, 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Absent subject folds to the empty string, never null.
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
    let input = SendInput {
        to: &to,
        subject: &subject,
        body: &text,
        cc: cc.as_deref(),
        bcc: bcc.as_deref(),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    Ok(
        match send_message(&state.pg, &sb, &user.id, now_ms(), &input).await {
            Ok((id, thread_id)) => {
                Json(json!({ "sent": { "id": id, "threadId": thread_id } })).into_response()
            }
            Err(e) => google_fail(e, "Gmail"),
        },
    )
}

// /api/brief/item. POST { sourceKey, action, tz } → check off, dismiss, or
// restore one brief
// line. The owner's own verdict on their own document — scoped to the
// caller's brief inside `mark_brief_item`, so a key belonging to somebody
// else's day resolves to no line rather than to theirs.
//
// The reader's timezone rides along because the check-off must land on the
// brief they are LOOKING at, which the fallback read may have served across
// a UTC midnight their timezone has not reached.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{enum_member, nullable_optional_string_member, string_member};
use talaria_daily_brief::{BriefUser, mark_brief_item, real_brief_deps};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// The POST body: sourceKey, action, tz.
struct ItemBody {
    source_key: String,
    action: String,
    tz: Option<String>,
}

fn validate(obj: &serde_json::Map<String, serde_json::Value>) -> Result<ItemBody, String> {
    let source_key = string_member(obj, "sourceKey", 1, 200)?;
    let action = enum_member(obj, "action", &["check", "dismiss", "restore"])?;
    // the helper answers the already-flattened Option (absent and null are
    // the same thing to the engine, which takes `tz: Option<&str>`).
    let tz = nullable_optional_string_member(obj, "tz", 64)?;
    Ok(ItemBody {
        source_key,
        action,
        tz,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let body = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let user = BriefUser::from(&user);
    let deps = real_brief_deps(&state).await;
    Ok(
        match mark_brief_item(
            &deps,
            &user,
            &body.source_key,
            &body.action,
            body.tz.as_deref(),
        )
        .await
        {
            Ok(mark) if mark.ok => Json(json!({ "ok": true })).into_response(),
            // 404 rather than 400: the request was well formed, the line just is
            // not on today's page — usually a stale tab from yesterday's brief.
            Ok(mark) => house_error(
                StatusCode::NOT_FOUND,
                mark.reason
                    .as_deref()
                    .unwrap_or("could not update that line"),
            ),
            Err(e) => internal("[brief] item mark failed", e),
        },
    )
}

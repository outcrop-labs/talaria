// /api/brief/item — port of ui/src/routes/api/brief.item.ts.
// POST { sourceKey, action, tz } → check off, dismiss, or restore one brief
// line. The owner's own verdict on their own document — scoped to the
// caller's brief inside `mark_brief_item`, so a key belonging to somebody
// else's day resolves to no line rather than to theirs.
//
// The reader's timezone rides along because the check-off must land on the
// brief they are LOOKING at, which the fallback read may have served across
// a UTC midnight their timezone has not reached.

use crate::body::{as_object, enum_member, nullable_optional_string_member, string_member};
use crate::daily_brief::{BriefUser, mark_brief_item, real_brief_deps};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The POST body, TS's `Body` zod shape.
struct ItemBody {
    source_key: String,
    action: String,
    /// `.nullable().optional()` — the tri-state a browser may or may not say.
    tz: Option<String>,
}

fn validate(obj: &serde_json::Map<String, serde_json::Value>) -> Result<ItemBody, String> {
    let source_key = string_member(obj, "sourceKey", 1, 200)?;
    let action = enum_member(obj, "action", &["check", "dismiss", "restore"])?;
    // `.nullable().optional()` — the helper answers the already-flattened
    // Option (absent and null are the same thing to the engine, which takes
    // `tz: Option<&str>`).
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
    let user = BriefUser::from(&user);
    let deps = real_brief_deps(&state).await;
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
        Err(e) => {
            tracing::error!("[brief] item mark failed: {e}");
            thrown_internal_error()
        }
    }
}

// /api/inbox/focus/state — port of ui/src/routes/api/inbox.focus.state.ts.
// PUT → mark a focus item viewed, or snooze it until a time. The lock is the
// whole route's: the state write and the snooze's decision row are one Inbox
// action, and a second one mid-flight is what the 409 refuses.

use crate::body::{
    as_object, enum_member, optional_boolean_member, present_nullable_datetime_member,
    string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::inbox_focus::update_focus_state;
use crate::inbox_focus_conversation::{acquire_inbox_focus_lock, record_inbox_snooze};
use crate::inbox_focus_types::FOCUS_SOURCE_TYPES;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The PUT body, TS's `Body` zod shape.
struct StateBody {
    source_type: String,
    source_id: String,
    snoozed_until: Option<Option<String>>,
    viewed: bool,
}

fn validate(obj: &serde_json::Map<String, Value>) -> Result<StateBody, String> {
    let source_type = enum_member(obj, "sourceType", &FOCUS_SOURCE_TYPES)?;
    let source_id = string_member(obj, "sourceId", 1, 500)?;
    let snoozed_until = present_nullable_datetime_member(obj, "snoozedUntil")?;
    let viewed = optional_boolean_member(obj, "viewed")?.unwrap_or(false);
    // `.refine((body) => body.snoozedUntil !== undefined || body.viewed,
    // 'state change required')` — present-null still counts as a change (it
    // clears the snooze); only absent-and-not-viewed is the empty request.
    if snoozed_until.is_none() && !viewed {
        return Err("state change required".into());
    }
    Ok(StateBody {
        source_type,
        source_id,
        snoozed_until,
        viewed,
    })
}

pub async fn put(
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
    let Some(_guard) = acquire_inbox_focus_lock(&user.id) else {
        return house_error(
            StatusCode::CONFLICT,
            "Your assistant is already handling another Inbox action.",
        );
    };
    let updated = match update_focus_state(
        &state.pg,
        &user,
        &body.source_type,
        &body.source_id,
        body.snoozed_until.as_ref().map(|o| o.as_deref()),
        body.viewed,
    )
    .await
    {
        Ok(ok) => ok,
        Err(e) => {
            tracing::error!("[inbox-focus] state write failed: {e}");
            return thrown_internal_error();
        }
    };
    if !updated {
        return house_error(
            StatusCode::CONFLICT,
            "That focus item is no longer available.",
        );
    }
    // `body.snoozedUntil ? … : null` — truthiness: only a present non-null
    // (the regex guarantees non-empty) value records the snooze decision row.
    let timeline_entry = match body.snoozed_until.as_ref().and_then(|o| o.as_deref()) {
        Some(snoozed_until) => match record_inbox_snooze(
            &state,
            &user,
            &body.source_type,
            &body.source_id,
            snoozed_until,
        )
        .await
        {
            Ok(entry) => entry,
            Err(e) => {
                tracing::error!("[inbox-focus] snooze record failed: {e}");
                return thrown_internal_error();
            }
        },
        None => None,
    };
    let mut ok = json!({ "ok": true });
    if let Some(entry) = timeline_entry
        && let Some(object) = ok.as_object_mut()
    {
        object.insert(
            "timelineEntry".into(),
            serde_json::to_value(&entry).expect("entry serializes"),
        );
    }
    (StatusCode::OK, Json(ok)).into_response()
}

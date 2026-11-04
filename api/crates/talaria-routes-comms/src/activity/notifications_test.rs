// POST /api/notifications/test. One test push to one of the caller's own
// devices, for the status panel: `{deviceId: uuid}` → `{ok, device: {id,
// status, ms, error}}`. 404 when the device is not the caller's; 429 with
// `Retry-After` and `{error, retryAfter}` past 5 calls in 10 minutes per
// person (a test push is a real push, and the button is easy to hold down);
// 501 while the push plane is absent or the test send is not built — it is a
// stub until W2-E (docs/notifications/delivery.md).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// Test pushes per person per window (O-20).
const TESTS_PER_WINDOW: i64 = 5;
const WINDOW_SECS: i64 = 600;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let device_id = match uuid_member(obj, "deviceId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match talaria_push::owns_device(&state.pg, &user.id, &device_id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[notifications/test] the device read failed", e)),
    }
    // The budget, counted before the send is even attempted: a stub that
    // answers 501 still spends it, so the limit is provable today. Fails open
    // on a Redis outage, like every house limiter.
    if let Ok(mut conn) = state.redis().await {
        let hit = talaria_ratelimit::rate_limit(
            &mut conn,
            &format!("notify:test:{}", user.id),
            TESTS_PER_WINDOW,
            WINDOW_SECS,
        )
        .await;
        if !hit.ok {
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": format!(
                        "too many test notifications — try again in {} seconds",
                        hit.retry_after
                    ),
                    "retryAfter": hit.retry_after,
                })),
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&hit.retry_after.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            return Ok(resp);
        }
    }
    if !talaria_push::push_plane_installed() {
        return Ok(house_error(
            StatusCode::NOT_IMPLEMENTED,
            "test notifications are not available on this server",
        ));
    }
    match talaria_push::send_test(&user.id, &device_id).await {
        Ok(t) => Ok(Json(json!({ "ok": true, "device": t.to_wire() })).into_response()),
        Err(talaria_push::TestError::NotFound) => {
            Ok(house_error(StatusCode::NOT_FOUND, "not found"))
        }
        Err(talaria_push::TestError::NotReady) => Ok(house_error(
            StatusCode::NOT_IMPLEMENTED,
            "test notifications are not available yet",
        )),
    }
}

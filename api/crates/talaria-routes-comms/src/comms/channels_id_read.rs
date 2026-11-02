// /api/channels/{id}/read.
// POST { seq } → advance the caller's read cursor (drives unread badges).
// A cursor that reaches the channel's newest message also clears the bell
// rows pointing at the channel — its DM row and any @mention in it — and
// tells the person's other tabs and devices which rows (`{type:
// "notifications", readIds}`), the same end a conversation read gives an
// agent thread's rows.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{NumKind, number_member};
use talaria_channels::{channel_role, mark_channel_read};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{NotifyDeps, clear_channel_notifications, publish_read};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[channels] role read on read-cursor failed", e)),
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    // seq: integer, min 0, no schema max — the ceiling is the safe-integer
    // bound itself.
    let seq = match number_member(obj, "seq", NumKind::Int, 0.0, 9_007_199_254_740_991.0) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // last_read_seq is int4: past 2^31-1 the write would die on Postgres'
    // own overflow (→ 500), so it is refused here.
    if seq > 2_147_483_647.0 {
        return Ok(internal("[channels] read cursor past int4", seq));
    }
    if let Err(e) = mark_channel_read(&state.pg, &id, &user.id, seq as i32).await {
        return Ok(internal("[channels] read-cursor advance failed", e));
    }
    // Caught up = the cursor (as stored — it only moves forward) covers the
    // newest complete message, thread replies included, the same messages
    // the unread badge counts. A failed sweep must not fail the read: the
    // cursor advanced, and the rows still clear from the bell.
    let mut cleared: Vec<String> = Vec::new();
    let caught_up: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "select coalesce((select last_read_seq from channel_members \
                          where channel_id = $1::uuid and user_id = $2::uuid), 0) \
             >= coalesce((select max(seq) from channel_messages \
                          where channel_id = $1::uuid and status = 'complete'), 0)",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_one(&state.pg)
    .await;
    match caught_up {
        Ok(true) => match clear_channel_notifications(&state.pg, &user.id, &id).await {
            Ok(n) => cleared = n,
            Err(e) => tracing::error!("[channels] notification sweep on read failed: {e}"),
        },
        Ok(false) => {}
        Err(e) => tracing::error!("[channels] caught-up read on read-cursor failed: {e}"),
    }
    if !cleared.is_empty() {
        let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        publish_read(&notify, &user.id, &cleared, false).await;
    }
    Ok(Json(json!({ "ok": true, "cleared": cleared.len() })).into_response())
}

// /api/channels/{id}/read.
// POST { seq?, threadRootId? } → advance the caller's read cursor (drives
// unread badges). Two arms, two cursors:
//
// - No `threadRootId`: the whole-channel cursor (`channel_members`).
//   `seq` required. A cursor that reaches the channel's newest message also
//   clears the bell rows pointing at the channel — its DM row and any
//   @mention in it — and tells the person's other tabs and devices which rows
//   (`{type: "notifications", readIds}`), the same end a conversation read
//   gives an agent thread's rows.
// - `threadRootId`: the ONE-thread cursor (`channel_thread_reads`) — reading
//   a thread is not reading the channel (the channel cursor advances only on
//   the main feed). `seq` optional: absent, the thread's newest complete reply
//   is computed here, so the panel can post the read the moment it opens.
//   Caught up = the thread cursor covers the thread's newest complete reply:
//   the thread's OWN bell rows clear (`clear_channel_thread_notifications`,
//   by group key — the href sweep would over-clear @mention rows sharing the
//   channel's href).
//
// The seqs are one space either way: replies share the channel's counter
// (`insert_channel_message`), so a thread cursor is comparable to reply seqs
// and the whole-channel caught-up gate still reads replies' seqs.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{NumKind, optional_number_member, optional_uuid_member};
use talaria_channels::{
    channel_role, get_channel_message, mark_channel_read, mark_channel_thread_read,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{
    NotifyDeps, clear_channel_notifications, clear_channel_thread_notifications, publish_read,
};
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
    let thread_root_id = match optional_uuid_member(obj, "threadRootId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // seq: integer, min 0, no schema max — the ceiling is the safe-integer
    // bound itself. Optional in the thread arm (computed server-side when
    // absent); required in the whole-channel arm.
    let seq = match optional_number_member(obj, "seq", NumKind::Int, 0.0, 9_007_199_254_740_991.0) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(s) = seq {
        // last_read_seq is int4: past 2^31-1 the write would die on
        // Postgres' own overflow (→ 500), so it is refused here.
        if s > 2_147_483_647.0 {
            return Ok(internal("[channels] read cursor past int4", s));
        }
    }
    let cleared: Vec<String> = if let Some(root_id) = thread_root_id {
        // A thread arm: re-root onto the thread's canonical root (Slack
        // semantics — threads never nest), and 404 a root that is not this
        // channel's rather than reading some other channel's cursor.
        let root = match get_channel_message(&state.pg, &id, &root_id).await {
            Ok(r) => match r {
                Some(r) => r,
                None => {
                    return Ok(house_error(
                        StatusCode::NOT_FOUND,
                        "no such message in this channel",
                    ));
                }
            },
            Err(e) => return Ok(internal("[channels] thread root read failed", e)),
        };
        let root_id = root.thread_root_id.unwrap_or(root.id);
        // Absent seq: the thread's newest complete reply — the same messages
        // the unread badge counts, so an open panel catches up in one post.
        let seq = match seq {
            Some(s) => s,
            None => {
                let s: i32 = sqlx::query_scalar(
                    "select coalesce((select max(seq) from channel_messages \
                     where thread_root_id = $1::uuid and status = 'complete'), 0)",
                )
                .bind(&root_id)
                .fetch_one(&state.pg)
                .await
                .map_err(|e| internal("[channels] thread latest read failed", e))?;
                s as f64
            }
        };
        if let Err(e) = mark_channel_thread_read(&state.pg, &root_id, &user.id, seq as i32).await {
            return Ok(internal("[channels] thread read-cursor advance failed", e));
        }
        // Caught up = the thread cursor (as stored — it only moves forward)
        // covers the thread's newest complete reply. A failed sweep must not
        // fail the read: the cursor advanced, and the rows still clear from
        // the bell.
        let caught_up: Result<bool, sqlx::Error> = sqlx::query_scalar(
            "select coalesce((select last_read_seq from channel_thread_reads \
                              where thread_root_id = $1::uuid and user_id = $2::uuid), 0) \
                 >= coalesce((select max(seq) from channel_messages \
                              where thread_root_id = $1::uuid and status = 'complete'), 0)",
        )
        .bind(&root_id)
        .bind(&user.id)
        .fetch_one(&state.pg)
        .await;
        let mut cleared: Vec<String> = Vec::new();
        match caught_up {
            Ok(true) => {
                match clear_channel_thread_notifications(&state.pg, &user.id, &root_id).await {
                    Ok(n) => cleared = n,
                    Err(e) => tracing::error!("[channels] thread sweep on read failed: {e}"),
                }
            }
            Ok(false) => {}
            Err(e) => tracing::error!("[channels] thread caught-up read failed: {e}"),
        }
        cleared
    } else {
        let seq = match seq {
            Some(s) => s,
            None => {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    "Invalid input: expected number, received undefined",
                ));
            }
        };
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
        cleared
    };
    if !cleared.is_empty() {
        let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        publish_read(&notify, &user.id, &cleared, false).await;
    }
    Ok(Json(json!({ "ok": true, "cleared": cleared.len() })).into_response())
}

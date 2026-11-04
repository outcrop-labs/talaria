// /api/channels/{id}/read.
// POST { seq?, threadRootId? } → advance the caller's read cursor (drives
// unread badges). Two arms, two cursors:
//
// - No `threadRootId`: the whole-channel cursor (`channel_members`).
//   `seq` required. Once the person is caught up on the whole channel, the
//   bell rows pointing at it clear — its DM row, any @mention in it, its
//   thread-reply rows — and the person's other tabs and devices hear which
//   rows (`{type: "notifications", readIds}`), the same end a conversation
//   read gives an agent thread's rows.
// - `threadRootId`: the ONE-thread cursor (`channel_thread_reads`) — reading
//   a thread is not reading the channel (the channel cursor advances only on
//   the main feed). `seq` optional: absent, the thread's newest complete reply
//   is computed here, so the panel can post the read the moment it opens.
//   Caught up on the thread (its cursor covers every complete reply): the
//   thread's OWN bell rows clear (`clear_channel_thread_notifications`, by
//   group key). The href sweep would over-clear: an @mention row shares the
//   channel's href, and a mention the main feed has not read is not read
//   because a reply was. When the thread read leaves the whole channel caught
//   up, the href sweep runs instead, since nothing on the channel is unread.
//
// "Caught up on the whole channel" is `channel_read_caught_up`: no complete
// message past the channel cursor, except replies their thread cursor covers.
// That is the badge's own reading of a thread read. Without it, a channel
// whose newest message is a reply could never catch up from the main feed
// (which never shows replies), and the badge would read clean beside a lit
// @mention. The seqs are one space: replies share the channel's counter
// (`insert_channel_message`), so the two cursors compare against one another.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{NumKind, optional_number_member, optional_uuid_member};
use talaria_channels::{
    channel_read_caught_up, channel_role, channel_thread_read_caught_up, get_channel_message,
    mark_channel_read, mark_channel_thread_read,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{
    NotifyDeps, clear_channel_notifications, clear_channel_thread_notifications, publish_read,
};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: Two arms, two cursors. Without `threadRootId`, the whole-channel
// doc: cursor (`seq` required); with it, that thread's own cursor (`seq`
// doc: optional: absent, the thread's newest complete reply). A root that is
// doc: not this channel's message is 404. The bell rows clear once the person
// doc: is caught up: the thread's own rows, or every row on the channel when
// doc: nothing on it is unread. `cleared` counts them.
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
    // The thread to read (any message in it re-roots to its root); absent,
    // the whole channel.
    let thread_root_id = match optional_uuid_member(obj, "threadRootId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Integer, min 0, ceiling the safe-integer bound. Required without
    // threadRootId; in a thread, absent reads to its newest complete reply.
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
        // Caught up on the thread = its cursor (as stored — it only moves
        // forward) covers every complete reply. If the whole channel is caught
        // up too, the channel's own sweep runs (every row on its href, this
        // thread's included): nothing on it is unread, so nothing over-clears.
        // Otherwise only the thread's rows clear. A failed gate or sweep must
        // not fail the read: the cursor advanced, and the rows still clear
        // from the bell.
        let mut cleared: Vec<String> = Vec::new();
        match channel_thread_read_caught_up(&state.pg, &root_id, &user.id).await {
            Ok(true) => {
                let whole = channel_read_caught_up(&state.pg, &id, &user.id)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!(
                            "[channels] channel caught-up read on thread read failed: {e}"
                        );
                        false
                    });
                let swept = if whole {
                    clear_channel_notifications(&state.pg, &user.id, &id).await
                } else {
                    clear_channel_thread_notifications(&state.pg, &user.id, &root_id).await
                };
                match swept {
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
        // Caught up = the cursor (as stored — it only moves forward) covers
        // every complete message, a reply its thread cursor covers counting as
        // read: the same messages the unread badge counts. A failed sweep must
        // not fail the read: the cursor advanced, and the rows still clear
        // from the bell.
        let mut cleared: Vec<String> = Vec::new();
        match channel_read_caught_up(&state.pg, &id, &user.id).await {
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

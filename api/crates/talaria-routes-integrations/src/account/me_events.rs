// GET /api/me/events → SSE stream of THIS person's own firehose: their runs
// changing state, their notifications landing, their brief being appended to.
//
// The per-device attach point. A run started on a laptop and parked on a
// question at 11pm has to reach the phone, and it has to reach it without the
// phone knowing which run to watch — which is the one thing `run:<id>` cannot
// do. Events here are id-shaped (see realtime's `UserEvent`); the client
// re-fetches through the ordinary routes, so this stream widens nothing.
//
// AUTHORIZATION IS THE SESSION. The topic is keyed by `user.id` taken from the
// resolved session and never from the request — no `?userId=`, no param, no
// route segment to tamper with. There is deliberately no way to ask for
// somebody else's firehose, which is why this route needs no check beyond
// being signed in: the only id it can construct is the caller's own.
//
// AND THE SESSION STAYS THE GATE (SE-16): the stream re-reads it before every
// frame and every 20 s, and ends on `session:revoked` (logout), each time
// with `{"type":"session","state":"ended"}`.
//
// RESUMING (I-19). `?since=<seq>` — or `Last-Event-ID: <seq>`, which an
// EventSource sends by itself on a reconnect; the query wins — replays what
// the stream missed: after `: connected`, the person's `presence`, then one
// `notification` frame (with `id: <seq>`) per row with a higher seq, in seq
// order, then `{"type":"resync","seq":N,"complete":true}`. Past 100 rows it
// sends only `{"type":"resync","seq":N,"complete":false}` and the client
// reads `GET /api/notifications?since=` itself. The topic is subscribed
// BEFORE the replay is read, and live frames the replay already covered are
// dropped, so each row arrives exactly once. Without a seq the stream opens
// as before (with the presence frame). The wire is
// docs/notifications/live-and-inbox.md.

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_notify::presence;
use talaria_realtime_watch::{
    RealtimeDeps, SESSION_REVOKED_TOPIC, SessionState, UserEvent, UserFrame, UserStream,
    resumable_event_stream, seq_now_fn, session_check_fn,
};
use talaria_session::{get_session_user, require_user, session_hash_of};
use talaria_state::AppState;

/// The seq to resume from: `?since=` first, then `Last-Event-ID`. Anything
/// that is not a whole number is no seq at all — the stream opens fresh
/// rather than refusing a header an older server never set.
fn since_of(raw_query: Option<&str>, headers: &HeaderMap) -> Option<i64> {
    let from_query = url::form_urlencoded::parse(raw_query.unwrap_or("").as_bytes())
        .find(|(k, _)| k == "since")
        .map(|(_, v)| v.into_owned());
    let raw = match from_query {
        Some(v) => v,
        None => headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())?
            .to_string(),
    };
    raw.trim().parse::<i64>().ok().filter(|n| *n >= 0)
}

/// The replay's frames (I-19): every row after `since` as a `notification`
/// frame carrying its seq, then the resync — or, past the cap, the resync
/// alone, incomplete.
fn replay_frames(r: &talaria_notify::Replay) -> Vec<UserFrame> {
    let mut out: Vec<UserFrame> = Vec::with_capacity(r.rows.len() + 1);
    if r.complete {
        out.extend(r.rows.iter().map(|(id, seq)| {
            UserFrame::of(&UserEvent::Notification {
                notification_id: id.clone(),
                seq: Some(*seq),
            })
        }));
    }
    out.push(UserFrame::of(&UserEvent::Resync {
        seq: r.seq,
        complete: r.complete,
    }));
    out
}

/// The `presence` frame for a `presence::snapshot` answer.
fn presence_frame(snap: &serde_json::Value) -> UserFrame {
    let name = |k: &str| snap.get(k).and_then(|v| v.as_str()).map(str::to_string);
    UserFrame::of(&UserEvent::Presence {
        looking: name("looking"),
        active: name("active"),
        clients: snap
            .get("clients")
            .and_then(|c| c.as_array())
            .map_or(0, |c| c.len() as u32),
    })
}

// doc: `?since=<seq>` or `Last-Event-ID` replays missed notification frames (each with `id: <seq>`), then `resync`; past 100 rows only `resync` with `complete: false`. Ends with `{"type":"session","state":"ended"}` when the session does. See docs/notifications/live-and-inbox.md.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let since = since_of(raw_query.as_deref(), &headers);
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);

    // SUBSCRIBE FIRST: the hub answers once Redis holds the topic, so a write
    // that commits after the replay's snapshot is heard live.
    let events = (deps.subscribe)(format!("user:{}", user.id)).await;
    let revoked = match session_hash_of(&headers) {
        Some(hash) => Some(((deps.subscribe)(SESSION_REVOKED_TOPIC.into()).await, hash)),
        None => None,
    };

    let mut preamble = Vec::new();
    // The person's presence as it stands: the frame is otherwise sent only
    // when the winner changes, and a stream that just opened has seen none.
    if let Ok(mut conn) = state.redis().await
        && let Ok(snap) = presence::snapshot(&mut conn, &user.id, wall_ms()).await
    {
        preamble.push(presence_frame(&snap));
    }
    let mut covered_through = None;
    if let Some(since) = since {
        match talaria_notify::replay(&state.pg, &user.id, since).await {
            Ok(r) => {
                covered_through = Some(r.seq);
                preamble.extend(replay_frames(&r));
            }
            Err(e) => {
                // The replay could not be read: say so, and the client reads
                // `?since` itself (and retries that read on its own failure).
                tracing::error!("[me/events] the replay read failed: {e}");
                preamble.push(UserFrame::of(&UserEvent::Resync {
                    seq: since,
                    complete: false,
                }));
            }
        }
    }

    // The session is re-read with nothing but its cookie: the stream must
    // not keep the whole request's headers for its lifetime.
    let mut cookie_only = HeaderMap::new();
    if let Some(c) = headers.get(header::COOKIE) {
        cookie_only.insert(header::COOKIE, c.clone());
    }
    let check_state = state.clone();
    let user_id = user.id.clone();
    let session = session_check_fn(move || {
        let state = check_state.clone();
        let headers = cookie_only.clone();
        let user_id = user_id.clone();
        async move {
            match get_session_user(&state, &headers).await {
                Ok(Some(u)) if u.id == user_id => SessionState::Alive,
                Ok(_) => SessionState::Ended,
                Err(e) => {
                    tracing::warn!("[me/events] the session re-read failed: {e}");
                    SessionState::Unknown
                }
            }
        }
    });
    let pg = state.pg.clone();
    let seq_user = user.id.clone();
    let seq_now = seq_now_fn(move || {
        let pg = pg.clone();
        let user_id = seq_user.clone();
        async move {
            talaria_notify::current_seq(&pg, &user_id)
                .await
                .ok()
                .flatten()
        }
    });

    let mut stream = UserStream::live(events);
    stream.revoked = revoked;
    stream.preamble = preamble;
    stream.covered_through = covered_through;
    stream.session = Some(session);
    stream.seq_now = Some(seq_now);
    Ok(resumable_event_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn h(id: Option<&str>) -> HeaderMap {
        let mut m = HeaderMap::new();
        if let Some(v) = id {
            m.insert("last-event-id", HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn the_resume_seq_is_the_query_then_last_event_id() {
        assert_eq!(since_of(Some("since=7"), &h(None)), Some(7));
        assert_eq!(since_of(None, &h(Some("9"))), Some(9));
        // The query wins over the header.
        assert_eq!(since_of(Some("since=3"), &h(Some("9"))), Some(3));
        assert_eq!(since_of(Some("since=0"), &h(None)), Some(0));
        // Not a seq: the stream opens fresh.
        assert_eq!(since_of(Some("since=abc"), &h(None)), None);
        assert_eq!(since_of(Some("since=-1"), &h(None)), None);
        assert_eq!(since_of(None, &h(Some(""))), None);
        assert_eq!(since_of(Some("other=1"), &h(None)), None);
        assert_eq!(since_of(None, &h(None)), None);
    }

    #[test]
    fn the_replay_is_its_rows_then_the_resync() {
        let r = talaria_notify::Replay {
            seq: 9,
            rows: vec![("a".into(), 8), ("b".into(), 9)],
            complete: true,
        };
        let f = replay_frames(&r);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].id, Some(8));
        assert_eq!(
            f[0].data,
            r#"{"type":"notification","notificationId":"a","seq":8}"#
        );
        assert_eq!(f[1].id, Some(9));
        assert_eq!(f[2].id, None);
        assert_eq!(f[2].data, r#"{"type":"resync","seq":9,"complete":true}"#);
        // Past the cap: the resync alone, incomplete.
        let over = talaria_notify::Replay {
            seq: 150,
            rows: Vec::new(),
            complete: false,
        };
        let f = replay_frames(&over);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].data, r#"{"type":"resync","seq":150,"complete":false}"#);
    }

    #[test]
    fn the_presence_frame_counts_the_clients() {
        let snap = json!({ "looking": "tab-1234abcd", "active": null, "clients": [{}, {}] });
        assert_eq!(
            presence_frame(&snap).data,
            r#"{"type":"presence","looking":"tab-1234abcd","active":null,"clients":2}"#
        );
    }
}

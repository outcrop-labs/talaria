// Live proof of the realtime hub, the stream's replay by seq and its session
// rules (docs/notifications/live-and-inbox.md), against a real Postgres and
// a real Redis: the subscriber count, PUBSUB NUMSUB, CLIENT KILL and the
// order of a subscribe against a commit are facts only Redis and Postgres
// can confirm. #[ignore]d, so the plain `cargo test` never needs either;
// CI's api-integration job runs them with --ignored.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test -p talaria-api --test it notify_hub_live:: -- --ignored

use crate::support::{call_json, live_app_state, minted_member_with_email, pg};
use futures_util::StreamExt;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use talaria_api::notify::{
    NotificationInput, NotifyDeps, add_notification_with, list_since, replay,
};
use talaria_api::realtime::{
    HUB_RECONNECTED, RealtimeDeps, UserEvent, UserFrame, UserStream, hub_refs, hub_topics,
    resumable_event_stream,
};

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// One test at a time: the hub is process-global, and a test that kills its
/// connection must not cost another test the messages it is counting.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn redis_url() -> String {
    std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)")
}

async fn redis() -> ConnectionManager {
    redis::Client::open(redis_url())
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap()
}

/// Every pubsub-mode client Redis holds right now: (id, subscriptions).
async fn pubsub_clients(conn: &mut ConnectionManager) -> Vec<(String, usize)> {
    let list: String = redis::cmd("CLIENT")
        .arg("LIST")
        .arg("TYPE")
        .arg("pubsub")
        .query_async(conn)
        .await
        .unwrap();
    list.lines()
        .filter_map(|l| {
            let field = |k: &str| {
                l.split(' ')
                    .find_map(|f| f.strip_prefix(k))
                    .map(str::to_string)
            };
            Some((field("id=")?, field("sub=")?.parse().ok()?))
        })
        .collect()
}

fn ids(clients: &[(String, usize)]) -> HashSet<String> {
    clients.iter().map(|(id, _)| id.clone()).collect()
}

/// Wait until this process's hub holds no topic and Redis has let go of
/// them: a previous test's streams are released asynchronously.
async fn hub_settled(url: &str) {
    for _ in 0..250 {
        if hub_topics(url) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn numsub(conn: &mut ConnectionManager, topic: &str) -> i64 {
    let v: (String, i64) = redis::cmd("PUBSUB")
        .arg("NUMSUB")
        .arg(topic)
        .query_async(conn)
        .await
        .unwrap();
    v.1
}

async fn publish(conn: &mut ConnectionManager, topic: &str, payload: &str) {
    redis::cmd("PUBLISH")
        .arg(topic)
        .arg(payload)
        .query_async::<i64>(conn)
        .await
        .unwrap();
}

fn topic_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

async fn cleanup(pg: &PgPool, tags: &[&str]) {
    let emails: Vec<String> = tags
        .iter()
        .map(|t| format!("notify-hub-{t}@test.invalid"))
        .collect();
    sqlx::query("delete from users where email = any($1)")
        .bind(&emails)
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, tag: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role, notify_prefs) \
         values ($1, $2, $3, 'member', $4::jsonb) returning id::text",
    )
    .bind(format!("notify-hub-{tag}"))
    .bind(format!("notify-hub-{tag}@test.invalid"))
    .bind(format!("Hub {tag}"))
    .bind(IN_APP)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn notify_seq(pg: &PgPool, user: &str) -> i64 {
    sqlx::query_scalar("select notify_seq from users where id = $1::uuid")
        .bind(user)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// File one mention for `user` through the real write path, publishing on
/// whatever Redis `deps` holds. Answers (id, seq).
async fn file(deps: &NotifyDeps, user: &str, title: &str) -> (String, i64) {
    let f = add_notification_with(
        deps,
        user,
        &NotificationInput {
            kind: "mention",
            title,
            body: Some("b"),
            href: Some("/hub-test"),
        },
        &Default::default(),
    )
    .await
    .unwrap();
    (f.id, f.seq)
}

/// Read an SSE body's frames until `done` holds or `ms` pass.
async fn frames_until(
    body: &mut axum::body::BodyDataStream,
    seen: &mut Vec<String>,
    ms: u64,
    done: impl Fn(&[String]) -> bool,
) {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut buf = String::new();
    while !done(seen) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        match tokio::time::timeout(left, body.next()).await {
            Ok(Some(Ok(bytes))) => {
                buf.push_str(std::str::from_utf8(&bytes).unwrap());
                while let Some(i) = buf.find("\n\n") {
                    seen.push(buf[..i].to_string());
                    buf.drain(..i + 2);
                }
            }
            _ => return,
        }
    }
}

fn mentions_of(frames: &[String], id: &str) -> usize {
    frames
        .iter()
        .filter(|f| f.contains(&format!("\"notificationId\":\"{id}\"")))
        .count()
}

// ── One connection, every stream ─────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live Redis (REDIS_URL)"]
async fn one_connection_carries_every_stream() {
    let _serial = SERIAL.lock().await;
    let url = redis_url();
    let mut conn = redis().await;
    let deps = RealtimeDeps::streams_only(&url);
    let users: Vec<String> = (0..10).map(|_| topic_id()).collect();
    let rooms: Vec<String> = (0..10)
        .map(|_| format!("board:{}", topic_id()))
        .chain((0..10).map(|_| format!("channel:{}", topic_id())))
        .collect();
    hub_settled(&url).await;
    let before = pubsub_clients(&mut conn).await;
    let mut user_rx = Vec::new();
    for u in &users {
        for _ in 0..5 {
            user_rx.push((u.clone(), (deps.subscribe)(format!("user:{u}")).await));
        }
    }
    let mut room_rx = Vec::new();
    for t in &rooms {
        room_rx.push((deps.subscribe)(t.clone()).await);
    }
    let after = pubsub_clients(&mut conn).await;
    // ONE connection holds all 30 topics: the only client with that many
    // subscriptions, and at most one more pubsub client than before (none,
    // when this process's hub was already subscribed to something).
    let holders: Vec<&(String, usize)> = after.iter().filter(|(_, n)| *n >= 30).collect();
    assert_eq!(
        holders.len(),
        1,
        "70 streams over 30 topics ride ONE subscriber connection: {after:?}"
    );
    assert!(
        ids(&after).difference(&ids(&before)).count() <= 1,
        "no connection per stream: {before:?} → {after:?}"
    );
    for u in &users {
        assert_eq!(
            numsub(&mut conn, &format!("user:{u}")).await,
            1,
            "five streams on one topic are one subscription"
        );
        assert_eq!(hub_refs(&url, &format!("user:{u}")), 5);
    }
    // Every stream still gets its own events, and only its own.
    for u in &users {
        publish(
            &mut conn,
            &format!("user:{u}"),
            &format!("{{\"type\":\"dm\",\"to\":\"{u}\"}}"),
        )
        .await;
    }
    let mut delivered = 0;
    for (u, rx) in user_rx.iter_mut() {
        let got = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("every stream hears its own user")
            .unwrap();
        assert!(got.contains(u.as_str()), "{got} on user:{u}");
        delivered += 1;
    }
    assert_eq!(delivered, 50);
    // Closing every stream unsubscribes every topic, at once.
    drop(user_rx);
    drop(room_rx);
    let closed = Instant::now();
    let all: Vec<String> = users
        .iter()
        .map(|u| format!("user:{u}"))
        .chain(rooms.iter().cloned())
        .collect();
    loop {
        let mut left = 0;
        for t in &all {
            left += numsub(&mut conn, t).await;
        }
        if left == 0 {
            break;
        }
        assert!(
            closed.elapsed() < Duration::from_secs(1),
            "{left} subscription(s) still held 1 s after every stream closed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(all.iter().all(|t| hub_refs(&url, t) == 0));
}

// ── A dropped connection comes back, and user streams resync ─────────────────

#[tokio::test]
#[ignore = "needs a live Redis (REDIS_URL)"]
async fn a_killed_connection_reconnects_and_tells_user_streams_to_resync() {
    let _serial = SERIAL.lock().await;
    let url = redis_url();
    let mut conn = redis().await;
    let deps = RealtimeDeps::streams_only(&url);
    let (user, board) = (topic_id(), topic_id());
    hub_settled(&url).await;
    let mut user_rx = (deps.subscribe)(format!("user:{user}")).await;
    let mut board_rx = (deps.subscribe)(format!("board:{board}")).await;
    // The hub's connection: the one pubsub client holding exactly our two
    // topics (this process's hub holds nothing else now).
    let after = pubsub_clients(&mut conn).await;
    let ours: Vec<&(String, usize)> = after.iter().filter(|(_, n)| *n == 2).collect();
    assert_eq!(
        ours.len(),
        1,
        "the hub's connection, and only it: {after:?}"
    );
    let hub_id = ours[0].0.clone();
    let killed = Instant::now();
    redis::cmd("CLIENT")
        .arg("KILL")
        .arg("ID")
        .arg(&hub_id)
        .query_async::<i64>(&mut conn)
        .await
        .unwrap();
    let marker = tokio::time::timeout(Duration::from_secs(5), user_rx.recv())
        .await
        .expect("the hub reconnects within 5 s")
        .unwrap();
    assert_eq!(marker, HUB_RECONNECTED);
    let took = killed.elapsed();
    assert_eq!(numsub(&mut conn, &format!("user:{user}")).await, 1);
    assert!(
        !ids(&pubsub_clients(&mut conn).await).contains(&hub_id),
        "a new connection, not the killed one"
    );
    // The next event arrives; a board stream is not asked to resync.
    publish(&mut conn, &format!("user:{user}"), r#"{"type":"dm"}"#).await;
    let next = tokio::time::timeout(Duration::from_secs(3), user_rx.recv())
        .await
        .expect("the next event arrives")
        .unwrap();
    assert_eq!(next, r#"{"type":"dm"}"#);
    publish(&mut conn, &format!("board:{board}"), r#"{"type":"task"}"#).await;
    let first = tokio::time::timeout(Duration::from_secs(3), board_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, r#"{"type":"task"}"#, "no marker on a board stream");
    eprintln!("hub reconnected {took:?} after CLIENT KILL");
}

// ── A write racing the replay arrives exactly once ───────────────────────────

/// Build the stream the route builds (subscribe, then replay), with the
/// write landing before (`write_first`) or after the replay's snapshot, and
/// count the frames that name the written row.
async fn race_once(pg: &PgPool, writes: &NotifyDeps, user: &str, write_first: bool) -> usize {
    let deps = RealtimeDeps::streams_only(&redis_url());
    let since = notify_seq(pg, user).await;
    let events = (deps.subscribe)(format!("user:{user}")).await;
    let mut written = None;
    if write_first {
        written = Some(file(writes, user, "before the snapshot").await);
    }
    let r = replay(pg, user, since).await.unwrap();
    if !write_first {
        written = Some(file(writes, user, "after the snapshot").await);
    }
    let (id, seq) = written.unwrap();
    let mut s = UserStream::live(events);
    s.covered_through = Some(r.seq);
    s.preamble = r
        .rows
        .iter()
        .map(|(id, seq)| {
            UserFrame::of(&UserEvent::Notification {
                notification_id: id.clone(),
                seq: Some(*seq),
            })
        })
        .chain(std::iter::once(UserFrame::of(&UserEvent::Resync {
            seq: r.seq,
            complete: true,
        })))
        .collect();
    let mut body = resumable_event_stream(s).into_body().into_data_stream();
    let mut seen = Vec::new();
    // Until the row has arrived (a slow disk delays the publish behind its
    // commit), then long enough for a duplicate to show up.
    let named = format!("\"notificationId\":\"{id}\"");
    frames_until(&mut body, &mut seen, 30_000, |f| {
        f.iter().any(|x| x.contains(&named))
    })
    .await;
    frames_until(&mut body, &mut seen, 500, |_| false).await;
    let frames: Vec<&String> = seen
        .iter()
        .filter(|f| f.contains(&format!("\"notificationId\":\"{id}\"")))
        .collect();
    assert!(
        frames
            .iter()
            .all(|f| f.starts_with(&format!("id: {seq}\n"))),
        "every frame of the row carries id: {seq}: {seen:?}"
    );
    frames.len()
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (DATABASE_URL, REDIS_URL)"]
async fn a_write_racing_the_replay_arrives_exactly_once() {
    let _serial = SERIAL.lock().await;
    let pg = pg().await;
    cleanup(&pg, &["race"]).await;
    let user = person(&pg, "race").await;
    let writes = NotifyDeps::publishing(pg.clone(), Some(redis().await));
    for run in 0..20 {
        let n = race_once(&pg, &writes, &user, run % 2 == 0).await;
        assert_eq!(n, 1, "run {run}: the row arrived {n} times");
    }
    cleanup(&pg, &["race"]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_long_gap_replays_nothing_and_says_read_since() {
    let _serial = SERIAL.lock().await;
    let pg = pg().await;
    cleanup(&pg, &["gap"]).await;
    let user = person(&pg, "gap").await;
    let quiet = NotifyDeps::publishing(pg.clone(), None);
    let since = notify_seq(&pg, &user).await;
    for i in 0..7 {
        file(&quiet, &user, &format!("seven {i}")).await;
    }
    let r = replay(&pg, &user, since).await.unwrap();
    assert!(r.complete);
    let seqs: Vec<i64> = r.rows.iter().map(|(_, s)| *s).collect();
    assert_eq!(seqs, ((since + 1)..=(since + 7)).collect::<Vec<_>>());
    assert_eq!(r.seq, since + 7);
    for i in 0..143 {
        file(&quiet, &user, &format!("many {i}")).await;
    }
    let r = replay(&pg, &user, since).await.unwrap();
    assert!(!r.complete, "150 rows is past the replay's 100");
    assert!(r.rows.is_empty());
    assert_eq!(r.seq, since + 150);
    let (rows, seq, more) = list_since(&pg, &user, since).await.unwrap();
    assert_eq!((rows.len(), seq, more), (150, since + 150, false));
    let seqs: Vec<i64> = rows.iter().map(|r| r["seq"].as_i64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "seq order");
    cleanup(&pg, &["gap"]).await;
}

// ── The route: replay over HTTP, and the session ends the stream ─────────────

async fn open_events(
    state: &talaria_api::state::AppState,
    cookie: &str,
    query: &str,
    last_event_id: Option<&str>,
) -> axum::body::BodyDataStream {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/api/me/events{query}"))
        .header("cookie", cookie);
    if let Some(id) = last_event_id {
        req = req.header("last-event-id", id);
    }
    let res = talaria_api::routes::router(state.clone())
        .oneshot(req.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.into_body().into_data_stream()
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (DATABASE_URL, REDIS_URL)"]
async fn the_route_replays_by_seq_and_ends_with_its_session() {
    let _serial = SERIAL.lock().await;
    let state = live_app_state().await;
    let email = "notify-hub-route@test.invalid";
    sqlx::query("delete from users where email = $1")
        .bind(email)
        .execute(&state.pg)
        .await
        .unwrap();
    let (user, cookie) = minted_member_with_email(&state, "notify-hub", email).await;
    let writes = NotifyDeps::publishing(state.pg.clone(), Some(redis().await));
    let since = notify_seq(&state.pg, &user).await;
    let mut ids = Vec::new();
    for i in 0..7 {
        ids.push(file(&writes, &user, &format!("missed {i}")).await);
    }
    let resync = format!(
        "data: {{\"type\":\"resync\",\"seq\":{},\"complete\":true}}",
        since + 7
    );
    let mut by_query = Vec::new();
    let mut a = open_events(&state, &cookie, &format!("?since={since}"), None).await;
    frames_until(&mut a, &mut by_query, 3_000, |f| f.contains(&resync)).await;
    let mut by_header = Vec::new();
    let mut b = open_events(&state, &cookie, "", Some(&since.to_string())).await;
    frames_until(&mut b, &mut by_header, 3_000, |f| f.contains(&resync)).await;
    assert_eq!(by_query, by_header, "Last-Event-ID replays the same frames");
    assert_eq!(by_query.len(), 10, "{by_query:?}");
    assert_eq!(by_query[0], ": connected\nretry: 3000");
    assert!(
        by_query[1].starts_with("data: {\"type\":\"presence\""),
        "{by_query:?}"
    );
    for ((id, seq), frame) in ids.iter().zip(&by_query[2..9]) {
        assert_eq!(
            *frame,
            format!(
                "id: {seq}\ndata: {{\"type\":\"notification\",\"notificationId\":\"{id}\",\"seq\":{seq}}}"
            )
        );
    }
    assert_eq!(by_query[9], resync);
    drop(b);

    // A second session of the same person keeps streaming while the first
    // signs out.
    let su = talaria_api::session::SessionUser {
        id: user.clone(),
        sub: "notify-hub-2".into(),
        email: Some(email.into()),
        name: None,
        picture: None,
        role: "member".into(),
        provider: "password".into(),
    };
    let sid2 = talaria_api::session::create_session(&state, &su)
        .await
        .unwrap();
    let cookie2 = format!("talaria_session={sid2}");
    let mut other = Vec::new();
    let mut c = open_events(&state, &cookie2, "", None).await;
    frames_until(&mut c, &mut other, 2_000, |f| f.len() >= 2).await;
    let (status, _) = call_json(&state, "POST", "/api/auth/logout", Some(&cookie), None).await;
    assert_eq!(status, 200);
    let ended = "data: {\"type\":\"session\",\"state\":\"ended\"}";
    let out = Instant::now();
    frames_until(&mut a, &mut by_query, 1_000, |f| {
        f.last().is_some_and(|x| x == ended)
    })
    .await;
    assert_eq!(
        by_query.last().unwrap(),
        ended,
        "the signed-out stream ends"
    );
    assert!(out.elapsed() <= Duration::from_secs(1));
    frames_until(&mut c, &mut other, 1_200, |f| f.iter().any(|x| x == ended)).await;
    assert!(
        !other.iter().any(|x| x == ended),
        "another session's stream lives"
    );

    // Its key deleted out from under it: no frame reaches it after.
    let mut conn = redis().await;
    redis::cmd("DEL")
        .arg(format!("sess:{sid2}"))
        .query_async::<i64>(&mut conn)
        .await
        .unwrap();
    let (late, _) = file(&writes, &user, "after the key went").await;
    frames_until(&mut c, &mut other, 3_000, |f| f.iter().any(|x| x == ended)).await;
    assert_eq!(other.last().unwrap(), ended);
    assert_eq!(
        mentions_of(&other, &late),
        0,
        "no frame after the DEL: {other:?}"
    );
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&user)
        .execute(&state.pg)
        .await
        .unwrap();
}

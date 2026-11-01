// Live-DB proof for the notification producers (docs/notifications/producers.md):
// rows coalesce by group key with a count, channel levels and timed mutes are
// obeyed, the people taking part in a conversation are told (each re-checked
// for access when the row is filed), and the rows about an approval share one
// key and read together when it is decided. Driven through the REAL router
// with minted sessions, so the whole path — the route, its detached fan-out,
// the writer's upsert — is what is proven. House rule: #[ignore]d; CI's
// api-integration job runs them with --ignored.
//
//   source ui/.env && cargo test -p talaria-api --test it notify_producers_live:: -- --ignored

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// One throwaway person: a name (the @mention tokens read it), every class
/// in-app (nothing is ever mailed), a minted session. (id, email, cookie).
async fn mint(state: &AppState, first: &str, role: &str) -> (String, String, String) {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let email = format!(
        "np-{}-{}@notify-producers.invalid",
        first.to_lowercase(),
        &tag[..12]
    );
    let sub = format!("notify-producers-live:{tag}");
    let name = format!("{first} Np{}", &tag[..6]);
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role, notify_prefs) \
         values ($1, $2, $3, $4, $5::jsonb) returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .bind(&name)
    .bind(role)
    .bind(IN_APP)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let user = talaria_api::session::SessionUser {
        id: id.clone(),
        sub,
        email: Some(email.clone()),
        name: Some(name),
        picture: None,
        role: role.into(),
        provider: "password".into(),
    };
    let sid = talaria_api::session::create_session(state, &user)
        .await
        .expect("session mints");
    (id, email, format!("talaria_session={sid}"))
}

/// The fan-outs are detached: poll until the condition holds (or 15 s pass).
/// A macro rather than a closure-taking function, so the condition can borrow
/// the test's locals freely.
macro_rules! eventually {
    ($cond:expr) => {{
        let mut held = false;
        for _ in 0..150 {
            if $cond {
                held = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        held
    }};
}

/// One row as these tests read it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    id: String,
    count: i32,
    group_key: Option<String>,
    alert: Option<String>,
    read: bool,
    created_ms: i64,
    title: String,
    body: String,
}

/// One person's rows of one kind at one href, oldest first.
async fn rows(state: &AppState, user: &str, kind: &str, href: &str) -> Vec<Row> {
    sqlx::query_as(
        "select id::text as id, count, group_key, alert, read_at is not null as read, \
                (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, title, body \
         from notifications where user_id = $1::uuid and kind = $2 and href = $3 \
         order by created_at, seq",
    )
    .bind(user)
    .bind(kind)
    .bind(href)
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

/// One person's rows about one entity.
async fn about(state: &AppState, user: &str, entity_id: &str) -> Vec<Row> {
    sqlx::query_as(
        "select id::text as id, count, group_key, alert, read_at is not null as read, \
                (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, title, body \
         from notifications where user_id = $1::uuid and entity_id = $2 order by created_at",
    )
    .bind(user)
    .bind(entity_id)
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

async fn ok(state: &AppState, method: &str, path: &str, cookie: &str, body: Value) -> Value {
    let body = (!body.is_null()).then_some(body);
    let (status, out) = call_json(state, method, path, Some(cookie), body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path} → {out}");
    out
}

fn id_of(v: &Value, key: &str) -> String {
    v[key]["id"].as_str().expect("an id").to_string()
}

async fn read_to_end(state: &AppState, channel: &str, cookie: &str) {
    let seq: i64 = sqlx::query_scalar("select msg_seq::bigint from channels where id = $1::uuid")
        .bind(channel)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    ok(
        state,
        "POST",
        &format!("/api/channels/{channel}/read"),
        cookie,
        json!({ "seq": seq }),
    )
    .await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_dm_coalesces_into_one_row_and_obeys_the_mute_and_the_level() {
    let state = live_app_state().await;
    let (ada, _, ada_c) = mint(&state, "Ada", "member").await;
    let (_bob, _, bob_c) = mint(&state, "Bob", "member").await;
    let dm = id_of(
        &ok(&state, "POST", "/api/dms", &bob_c, json!({ "userId": ada })).await,
        "channel",
    );
    let href = format!("/comms/channel/{dm}");
    let post = format!("/api/channels/{dm}/messages");
    let notify = format!("/api/channels/{dm}/notify");

    // Three messages → ONE row, count 3, the newest line, createdAt rising.
    let mut created = Vec::new();
    for i in 1..=3 {
        ok(
            &state,
            "POST",
            &post,
            &bob_c,
            json!({ "content": format!("line {i}") }),
        )
        .await;
        assert!(eventually!(
            rows(&state, &ada, "dm", &href)
                .await
                .first()
                .is_some_and(|r| r.count == i)
        ));
        created.push(rows(&state, &ada, "dm", &href).await[0].created_ms);
    }
    let r = rows(&state, &ada, "dm", &href).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].group_key.as_deref(), Some(format!("dm:{dm}").as_str()));
    assert_eq!(r[0].body, "line 3");
    assert!(created.windows(2).all(|w| w[1] > w[0]), "{created:?}");

    // Read to the end → the next message files a fresh row.
    read_to_end(&state, &dm, &ada_c).await;
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "line 4" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &ada, "dm", &href).await.len() == 2
    ));
    let r = rows(&state, &ada, "dm", &href).await;
    assert!(r[0].read && !r[1].read && r[1].count == 1 && r[1].id != r[0].id);

    // Muted for an hour: a feed row. Unmuted: a banner again.
    let until =
        talaria_api::agent_auth::epoch_ms_to_iso(talaria_api::agent_auth::now_ms() + 3_600_000);
    let put = ok(
        &state,
        "PUT",
        &notify,
        &ada_c,
        json!({ "mutedUntil": until }),
    )
    .await;
    assert_eq!(put["level"], "all", "a DM defaults to all");
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "muted line" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &ada, "dm", &href)
            .await
            .get(1)
            .is_some_and(|r| r.count == 2)
    ));
    assert_eq!(
        rows(&state, &ada, "dm", &href).await[1].alert.as_deref(),
        Some("feed")
    );
    let detail = ok(
        &state,
        "GET",
        &format!("/api/channels/{dm}"),
        &ada_c,
        Value::Null,
    )
    .await;
    assert_eq!(detail["notify"]["mutedUntil"], json!(until));
    ok(
        &state,
        "PUT",
        &notify,
        &ada_c,
        json!({ "mutedUntil": null }),
    )
    .await;
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "loud again" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &ada, "dm", &href)
            .await
            .get(1)
            .is_some_and(|r| r.count == 3)
    ));
    assert_eq!(
        rows(&state, &ada, "dm", &href).await[1].alert.as_deref(),
        Some("banner")
    );

    // Level `none` files nothing at all.
    ok(&state, "PUT", &notify, &ada_c, json!({ "level": "none" })).await;
    read_to_end(&state, &dm, &ada_c).await;
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "into the void" }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        rows(&state, &ada, "dm", &href).await.iter().all(|r| r.read),
        "level none filed a row"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_pre_contract_dm_row_is_adopted_not_doubled() {
    let state = live_app_state().await;
    let (ada, _, _) = mint(&state, "Ada", "member").await;
    let (_eve, _, eve_c) = mint(&state, "Eve", "member").await;
    let dm = id_of(
        &ok(&state, "POST", "/api/dms", &eve_c, json!({ "userId": ada })).await,
        "channel",
    );
    let href = format!("/comms/channel/{dm}");
    // Wave 1's row: no key, no seq.
    let (legacy,): (String,) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href) \
         values ($1::uuid, 'dm', 'Eve sent you a message', 'wave one', $2) returning id::text",
    )
    .bind(&ada)
    .bind(&href)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    ok(
        &state,
        "POST",
        &format!("/api/channels/{dm}/messages"),
        &eve_c,
        json!({ "content": "after" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &ada, "dm", &href)
            .await
            .first()
            .is_some_and(|r| r.count == 2)
    ));
    let r = rows(&state, &ada, "dm", &href).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].id, legacy, "the same id — a snooze on it stays");
    assert_eq!(r[0].group_key.as_deref(), Some(format!("dm:{dm}").as_str()));
}

/// A post with no text — a file on its own — rings the other side of a DM
/// exactly as a line would (wave 1 filed a DM row for it), and the row says
/// what it was.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_attachment_only_dm_rings_like_a_line_of_text() {
    let state = live_app_state().await;
    let (ada, _, _) = mint(&state, "Ada", "member").await;
    let (bob, _, bob_c) = mint(&state, "Bob", "member").await;
    let dm = id_of(
        &ok(&state, "POST", "/api/dms", &bob_c, json!({ "userId": ada })).await,
        "channel",
    );
    let href = format!("/comms/channel/{dm}");
    let (upload,): (String,) = sqlx::query_as(
        "insert into uploads (filename, mime, size, path, uploaded_by) \
         values ('np.txt', 'text/plain', 3, '/nonexistent/np.txt', $1::uuid) returning id::text",
    )
    .bind(&bob)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    ok(
        &state,
        "POST",
        &format!("/api/channels/{dm}/messages"),
        &bob_c,
        json!({ "content": "", "attachmentIds": [upload] }),
    )
    .await;
    assert!(
        eventually!(!rows(&state, &ada, "dm", &href).await.is_empty()),
        "an attachment-only DM filed no row"
    );
    let r = rows(&state, &ada, "dm", &href).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].body, "Sent an attachment");
    assert!(!r[0].read);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn channel_levels_file_what_they_say() {
    let state = live_app_state().await;
    let (ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (_bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let ch = id_of(
        &ok(
            &state,
            "POST",
            "/api/channels",
            &ada_c,
            json!({ "name": format!("np-{}", &ada[..8]) }),
        )
        .await,
        "channel",
    );
    for e in [&bob_e, &cy_e] {
        ok(
            &state,
            "POST",
            &format!("/api/channels/{ch}/members"),
            &ada_c,
            json!({ "email": e }),
        )
        .await;
    }
    let href = format!("/comms/channel/{ch}");
    let post = format!("/api/channels/{ch}/messages");
    // The default, mentions: a plain message files nothing.
    let detail = ok(
        &state,
        "GET",
        &format!("/api/channels/{ch}"),
        &ada_c,
        Value::Null,
    )
    .await;
    assert_eq!(
        detail["notify"],
        json!({ "level": "mentions", "mutedUntil": null, "settable": true })
    );
    ok(&state, "POST", &post, &bob_c, json!({ "content": "plain" })).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        rows(&state, &ada, "channel-message", &href)
            .await
            .is_empty()
    );
    // All: ONE channel-message row per channel, counting quietly.
    ok(
        &state,
        "PUT",
        &format!("/api/channels/{ch}/notify"),
        &ada_c,
        json!({ "level": "all" }),
    )
    .await;
    let mut created = Vec::new();
    for i in 1..=3 {
        ok(
            &state,
            "POST",
            &post,
            &bob_c,
            json!({ "content": format!("all {i}") }),
        )
        .await;
        assert!(eventually!(
            rows(&state, &ada, "channel-message", &href)
                .await
                .first()
                .is_some_and(|r| r.count == i)
        ));
        created.push(rows(&state, &ada, "channel-message", &href).await[0].created_ms);
    }
    let r = rows(&state, &ada, "channel-message", &href).await;
    assert_eq!(r.len(), 1);
    assert_eq!(
        r[0].group_key.as_deref(),
        Some(format!("channel:{ch}:activity").as_str())
    );
    assert!(
        created.windows(2).all(|w| w[1] == w[0]),
        "quiet: createdAt stays {created:?}"
    );
    assert!(
        rows(&state, &cy, "channel-message", &href).await.is_empty(),
        "Cy is at mentions"
    );
    // None: an @mention files nothing; back at the default it does.
    ok(
        &state,
        "PUT",
        &format!("/api/channels/{ch}/notify"),
        &cy_c,
        json!({ "level": "none" }),
    )
    .await;
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "@cy hello?" }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        rows(&state, &cy, "mention", &href).await.is_empty(),
        "none silences @mentions"
    );
    ok(
        &state,
        "PUT",
        &format!("/api/channels/{ch}/notify"),
        &cy_c,
        json!({ "level": null }),
    )
    .await;
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "@cy now?" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &cy, "mention", &href).await.len() == 1
    ));
    // A non-member cannot set a level.
    let (_dan, _, dan_c) = mint(&state, "Dan", "member").await;
    let (status, _) = call_json(
        &state,
        "PUT",
        &format!("/api/channels/{ch}/notify"),
        Some(&dan_c),
        Some(json!({ "level": "none" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn status_moves_coalesce_quietly_and_the_creator_hears_them() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let board = id_of(
        &ok(
            &state,
            "POST",
            "/api/boards",
            &ada_c,
            json!({ "name": "np status" }),
        )
        .await,
        "board",
    );
    ok(
        &state,
        "POST",
        &format!("/api/boards/{board}/members"),
        &ada_c,
        json!({ "email": bob_e, "role": "editor" }),
    )
    .await;
    let task = id_of(
        &ok(
            &state,
            "POST",
            &format!("/api/boards/{board}/tasks"),
            &bob_c,
            json!({ "title": "Burst" }),
        )
        .await,
        "task",
    );
    let href = format!("/boards/{board}/{task}");
    for status in ["assigned", "in_progress", "assigned", "in_progress"] {
        ok(
            &state,
            "PUT",
            &format!("/api/tasks/{task}"),
            &ada_c,
            json!({ "status": status }),
        )
        .await;
    }
    assert!(eventually!(
        rows(&state, &bob, "task-status", &href)
            .await
            .first()
            .is_some_and(|r| r.count == 4)
    ));
    let r = rows(&state, &bob, "task-status", &href).await;
    assert_eq!(r.len(), 1, "the creator hears the moves, once");
    assert_eq!(r[0].body, "Moved 4 times — now in progress");
    // Past the window (the row last changed eleven minutes ago): a fresh row,
    // and the old one keeps its content and loses its key.
    sqlx::query(
        "update notifications set updated_at = now() - interval '11 minutes' where id = $1::uuid",
    )
    .bind(&r[0].id)
    .execute(&state.pg)
    .await
    .unwrap();
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{task}"),
        &ada_c,
        json!({ "status": "done" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &bob, "task-status", &href).await.len() == 2
    ));
    let r = rows(&state, &bob, "task-status", &href).await;
    assert_eq!((r[0].count, r[0].group_key.as_deref()), (4, None));
    assert_eq!(r[1].count, 1);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn comments_and_thread_replies_reach_the_people_taking_part() {
    let state = live_app_state().await;
    let (ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let (eve, eve_e, eve_c) = mint(&state, "Eve", "member").await;
    let board = id_of(
        &ok(
            &state,
            "POST",
            "/api/boards",
            &ada_c,
            json!({ "name": "np people" }),
        )
        .await,
        "board",
    );
    for e in [&bob_e, &cy_e, &eve_e] {
        ok(
            &state,
            "POST",
            &format!("/api/boards/{board}/members"),
            &ada_c,
            json!({ "email": e, "role": "editor" }),
        )
        .await;
    }
    let task = id_of(
        &ok(
            &state,
            "POST",
            &format!("/api/boards/{board}/tasks"),
            &eve_c,
            json!({ "title": "People", "assignees": [format!("user:{bob}")] }),
        )
        .await,
        "task",
    );
    let href = format!("/boards/{board}/{task}");
    let comments = format!("/api/tasks/{task}/comments");
    ok(
        &state,
        "POST",
        &comments,
        &cy_c,
        json!({ "content": "Cy was here" }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    sqlx::query("delete from notifications where href = $1 and kind = 'task-comment'")
        .bind(&href)
        .execute(&state.pg)
        .await
        .unwrap();
    // Cy leaves the board; Ada comments.
    ok(
        &state,
        "DELETE",
        &format!("/api/boards/{board}/members"),
        &ada_c,
        json!({ "userId": cy }),
    )
    .await;
    ok(
        &state,
        "POST",
        &comments,
        &ada_c,
        json!({ "content": "Ada weighs in" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &bob, "task-comment", &href).await.len() == 1
            && rows(&state, &eve, "task-comment", &href).await.len() == 1
    ));
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        rows(&state, &ada, "task-comment", &href).await.is_empty(),
        "never the author"
    );
    assert!(
        rows(&state, &cy, "task-comment", &href).await.is_empty(),
        "removed from the board: nothing (C24)"
    );
    // A second comment coalesces; a comment that @mentions Bob leaves him to
    // his mention row.
    ok(
        &state,
        "POST",
        &comments,
        &ada_c,
        json!({ "content": "and another" }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &eve, "task-comment", &href)
            .await
            .first()
            .is_some_and(|r| r.count == 2)
    ));
    let bob_name: String = sqlx::query_scalar("select name from users where id = $1::uuid")
        .bind(&bob)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    let first = bob_name.split_whitespace().next().unwrap().to_lowercase();
    let held = rows(&state, &bob, "task-comment", &href).await[0].count;
    ok(
        &state,
        "POST",
        &comments,
        &ada_c,
        json!({ "content": format!("@{first} please") }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &eve, "task-comment", &href)
            .await
            .first()
            .is_some_and(|r| r.count == 3)
    ));
    assert!(eventually!(
        rows(&state, &bob, "mention", &href).await.len() == 1
    ));
    assert_eq!(
        rows(&state, &bob, "task-comment", &href).await[0].count,
        held,
        "mentioned: no task-comment on top"
    );

    // Thread replies: the root's author and the earlier repliers.
    let ch = id_of(
        &ok(
            &state,
            "POST",
            "/api/channels",
            &ada_c,
            json!({ "name": format!("np-t-{}", &ada[..8]) }),
        )
        .await,
        "channel",
    );
    for e in [&bob_e, &cy_e] {
        ok(
            &state,
            "POST",
            &format!("/api/channels/{ch}/members"),
            &ada_c,
            json!({ "email": e }),
        )
        .await;
    }
    let post = format!("/api/channels/{ch}/messages");
    let chref = format!("/comms/channel/{ch}");
    let root = id_of(
        &ok(&state, "POST", &post, &ada_c, json!({ "content": "root" })).await,
        "message",
    );
    ok(
        &state,
        "POST",
        &post,
        &bob_c,
        json!({ "content": "bob replies", "threadRootId": root }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &ada, "thread-reply", &chref).await.len() == 1
    ));
    ok(
        &state,
        "POST",
        &post,
        &cy_c,
        json!({ "content": "cy too", "threadRootId": root }),
    )
    .await;
    assert!(eventually!(
        rows(&state, &bob, "thread-reply", &chref).await.len() == 1
    ));
    assert!(eventually!(
        rows(&state, &ada, "thread-reply", &chref)
            .await
            .first()
            .is_some_and(|r| r.count == 2)
    ));
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        rows(&state, &cy, "thread-reply", &chref).await.is_empty(),
        "never told of their own reply"
    );
    let ada_rows = rows(&state, &ada, "thread-reply", &chref).await;
    assert_eq!(ada_rows.len(), 1);
    assert_eq!(
        ada_rows[0].group_key.as_deref(),
        Some(format!("reply:{root}").as_str())
    );
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_approval_is_one_row_per_person_read_by_the_decision_and_realerted_by_the_sla() {
    let state = live_app_state().await;
    let (ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, _) = mint(&state, "Cy", "member").await;
    let (dan, _, dan_c) = mint(&state, "Dan", "member").await;
    let board = id_of(
        &ok(
            &state,
            "POST",
            "/api/boards",
            &ada_c,
            json!({ "name": "np approval" }),
        )
        .await,
        "board",
    );
    for e in [&bob_e, &cy_e] {
        ok(
            &state,
            "POST",
            &format!("/api/boards/{board}/members"),
            &ada_c,
            json!({ "email": e, "role": "editor" }),
        )
        .await;
    }
    let model = format!("np-pa-{}", &dan[..8]);
    sqlx::query(
        "insert into agent_defs (slug, department, model, display_name, owner_user_id) \
         values ($1, 'personal', $1, 'Np PA', $2::uuid)",
    )
    .bind(&model)
    .bind(&dan)
    .execute(&state.pg)
    .await
    .unwrap();
    ok(
        &state,
        "POST",
        &format!("/api/boards/{board}/agent-requests"),
        &dan_c,
        json!({ "agentModel": model }),
    )
    .await;
    let key = format!("board_access:{board}:{model}");
    for u in [&ada, &bob, &cy] {
        assert!(
            eventually!(about(&state, u, &key).await.len() == 1),
            "{u} was told"
        );
        assert_eq!(
            about(&state, u, &key).await[0].group_key.as_deref(),
            Some(format!("approval:{key}").as_str())
        );
    }
    ok(
        &state,
        "PUT",
        &format!("/api/boards/{board}/agent-requests"),
        &bob_c,
        json!({ "agentModel": model, "action": "approve" }),
    )
    .await;
    for u in [&ada, &bob, &cy] {
        assert!(
            eventually!(about(&state, u, &key).await[0].read),
            "{u}'s row read by the decision"
        );
    }

    // The SLA: a ticket in review with Bob on it — announced, then nagged into
    // THE SAME row (the job's own function, its clock moved past the nag).
    let task = id_of(
        &ok(
            &state,
            "POST",
            &format!("/api/boards/{board}/tasks"),
            &ada_c,
            json!({ "title": "Sign me off", "assignees": [format!("user:{bob}")] }),
        )
        .await,
        "task",
    );
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{task}"),
        &ada_c,
        json!({ "status": "quality_review" }),
    )
    .await;
    let review = format!("ticket_review:{task}");
    let deps = talaria_digest::real_digest_deps(
        &state,
        talaria_api::realtime::RealtimeDeps::publish_only(None),
    );
    let now = talaria_api::agent_auth::now_ms();
    talaria_digest::run_approval_escalation(&deps, now)
        .await
        .unwrap();
    assert!(
        eventually!(about(&state, &bob, &review).await.len() == 1),
        "announced to its owner"
    );
    let announced = about(&state, &bob, &review).await[0].clone();
    // Past the nag, short of the escalation — midway between the two
    // thresholds the instance is configured with (four hours and a day by
    // default). A pass spends a capped budget oldest-first, so another
    // test's backlog may take a few passes to clear before this one is
    // reached.
    let config = talaria_digest::digest_config(&state.pg).await;
    let later =
        now + ((config.nag_after_minutes + config.escalate_after_minutes) / 2.0 * 60_000.0) as i64;
    let mut nagged = Vec::new();
    for _ in 0..20 {
        talaria_digest::run_approval_escalation(&deps, later)
            .await
            .unwrap();
        nagged = about(&state, &bob, &review).await;
        if nagged.first().is_some_and(|r| r.count == 2) {
            break;
        }
    }
    assert_eq!(nagged.len(), 1, "{nagged:?}");
    assert_eq!(nagged[0].id, announced.id, "the nag re-alerts the same row");
    assert_eq!(nagged[0].count, 2);
    assert!(
        nagged[0].title.starts_with("Still waiting on you"),
        "{}",
        nagged[0].title
    );
    assert!(
        nagged[0].created_ms > announced.created_ms,
        "and moves its createdAt (a re-alert)"
    );
    // Signed off by moving it out of review: the row reads.
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{task}"),
        &ada_c,
        json!({ "status": "done" }),
    )
    .await;
    assert!(
        eventually!(about(&state, &bob, &review).await[0].read),
        "leaving review reads it"
    );
}

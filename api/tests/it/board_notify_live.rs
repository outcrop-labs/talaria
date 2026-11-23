// Live-DB proof for per-board notification settings (talaria_notify::boards,
// docs/notifications/producers.md "Each board"): with nobody's setting stored
// every ticket producer files exactly the rows it filed before — the people in
// a ticket hear it, a member who is not hears nothing — and a setting changes
// what THAT person receives and nobody else: Off silences a board (a watch the
// person made themselves still reaches them; one an editor added does not),
// All files one counting board-activity row, a custom set without @mentions
// keeps the comment, one action is one row (a patch that assigns and moves; a
// new ticket that @mentions a whole-board listener), the board-wide probe
// hears both a row and a default, the setting survives leaving and
// rejoining a board, and a board's own channels shape that person's rows and
// nobody else's (stored on each row for the send-time re-checks).
// Driven through the REAL router with minted sessions, so the route, its
// detached fan-out and the writer's upsert are what is proven. House rule:
// #[ignore]d; ci.yml's api job runs them with --ignored (its live-DB steps).
//
//   source ui/.env && cargo test -p talaria-api --test it board_notify_live:: -- --ignored

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// One throwaway person: a name (the @mention tokens read it), every class
/// in-app (nothing is ever mailed), a minted session. (id, email, cookie,
/// the first-name token they answer to).
async fn mint(state: &AppState, first: &str, role: &str) -> (String, String, String, String) {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let email = format!(
        "bn-{}-{}@board-notify.invalid",
        first.to_lowercase(),
        &tag[..12]
    );
    let sub = format!("board-notify-live:{tag}");
    // A first name of its own, so no other test's person answers to it.
    let given = format!("{first}{}", &tag[..6]);
    let name = format!("{given} Bn");
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
    (
        id,
        email,
        format!("talaria_session={sid}"),
        given.to_lowercase(),
    )
}

/// The fan-outs are detached: poll until the condition holds (or 15 s pass).
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

/// Long enough for a detached fan-out that should file nothing to have filed
/// it, before asserting that it did not.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    id: String,
    kind: String,
    count: i32,
    group_key: Option<String>,
    alert: Option<String>,
    entity_type: Option<String>,
    body: String,
}

/// Every row one person holds that is about this board or one of its
/// tickets, oldest first.
async fn on_board(state: &AppState, user: &str, board: &str) -> Vec<Row> {
    sqlx::query_as(
        "select id::text as id, kind, count, group_key, alert, entity_type, body \
         from notifications \
         where user_id = $1::uuid and (href like $2 or href = $3 \
               or entity_id in (select id::text from tasks where board_id = $4::uuid)) \
         order by created_at, seq",
    )
    .bind(user)
    .bind(format!("/boards/{board}/%"))
    .bind(format!("/boards/{board}"))
    .bind(board)
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

fn kinds(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|r| r.kind.as_str()).collect()
}

/// The kinds, sorted: for steps whose detached fan-outs may land in any
/// order.
fn kind_set(rows: &[Row]) -> Vec<&str> {
    let mut k = kinds(rows);
    k.sort_unstable();
    k
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

async fn board(state: &AppState, owner: &str, name: &str, members: &[(&str, &str)]) -> String {
    let b = id_of(
        &ok(state, "POST", "/api/boards", owner, json!({ "name": name })).await,
        "board",
    );
    for (email, role) in members {
        ok(
            state,
            "POST",
            &format!("/api/boards/{b}/members"),
            owner,
            json!({ "email": email, "role": role }),
        )
        .await;
    }
    b
}

async fn ticket(state: &AppState, board: &str, cookie: &str, body: Value) -> String {
    id_of(
        &ok(
            state,
            "POST",
            &format!("/api/boards/{board}/tasks"),
            cookie,
            body,
        )
        .await,
        "task",
    )
}

async fn set(state: &AppState, board: &str, cookie: &str, body: Value) -> Value {
    ok(
        state,
        "PUT",
        &format!("/api/boards/{board}/notify"),
        cookie,
        body,
    )
    .await
}

// EXISTING USERS SEE NO CHANGE. Nobody on the board has a setting: the
// assignee hears the assignment, the move and the comment, the person a
// comment @mentions gets the mention, and a member who is in none of it gets
// nothing — no board-activity row, no new-ticket row, nothing — exactly the
// fan-out before boards had a setting.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn with_no_setting_every_producer_files_what_it_did() {
    let state = live_app_state().await;
    let (ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, _) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, _, cy_tok) = mint(&state, "Cy", "member").await;
    let (dee, dee_e, _, _) = mint(&state, "Dee", "member").await;
    let b = board(
        &state,
        &ada_c,
        "bn parity",
        &[(&bob_e, "editor"), (&cy_e, "editor"), (&dee_e, "viewer")],
    )
    .await;
    let t = ticket(
        &state,
        &b,
        &ada_c,
        json!({ "title": "Parity", "assignees": [format!("user:{bob}")] }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-assigned"]
    ));
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-assigned", "task-status"]
    ));
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t}/comments"),
        &ada_c,
        json!({ "content": format!("@{cy_tok} have a look") }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await)
            == ["task-assigned", "task-status", "task-comment"]
    ));
    assert!(eventually!(
        kinds(&on_board(&state, &cy, &b).await) == ["mention"]
    ));
    // Done: a completion is a status row, as it always was.
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "status": "done" }),
    )
    .await;
    assert!(eventually!(
        on_board(&state, &bob, &b)
            .await
            .iter()
            .any(|r| r.kind == "task-status" && r.count == 2)
    ));
    settle().await;
    assert!(
        on_board(&state, &dee, &b).await.is_empty(),
        "a member in none of it hears nothing"
    );
    assert!(
        on_board(&state, &ada, &b).await.is_empty(),
        "never the actor"
    );
    assert_eq!(
        kinds(&on_board(&state, &cy, &b).await),
        ["mention"],
        "the mentioned person: the mention, never a comment row on top"
    );
    let activity: i64 = sqlx::query_scalar(
        "select count(*) from notifications where kind = 'board-activity' and href = $1",
    )
    .bind(format!("/boards/{b}"))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(activity, 0, "nobody chose to hear the board");
    // And the setting reads as the built-in.
    let got = ok(
        &state,
        "GET",
        &format!("/api/boards/{b}/notify"),
        &bob_c,
        Value::Null,
    )
    .await;
    assert_eq!(got["source"], "builtin");
    assert_eq!(got["effective"]["preset"], "participating");
}

// OFF silences the board for that person — assignments and @mentions
// included — and for nobody else. A watch the person made themselves still
// carries its ticket's news; a watch an editor added for them does not,
// until they press Watch themselves.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn off_silences_the_board_for_that_person_only() {
    let state = live_app_state().await;
    let (ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, bob_tok) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, _, _) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "bn off",
        &[(&bob_e, "editor"), (&cy_e, "editor")],
    )
    .await;
    let got = set(&state, &b, &bob_c, json!({ "preset": "off" })).await;
    assert_eq!(
        (got["source"].as_str(), got["effective"]["preset"].as_str()),
        (Some("board"), Some("off"))
    );
    // Assigned to both, @mentioning Bob, moved, commented.
    let t1 = ticket(
        &state,
        &b,
        &ada_c,
        json!({
            "title": "Both",
            "description": format!("@{bob_tok} this is yours too"),
            "assignees": [format!("user:{bob}"), format!("user:{cy}")],
        }),
    )
    .await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t1}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t1}/comments"),
        &ada_c,
        json!({ "content": format!("@{bob_tok} please") }),
    )
    .await;
    assert!(eventually!(
        kind_set(&on_board(&state, &cy, &b).await)
            == ["task-assigned", "task-comment", "task-status"]
    ));
    settle().await;
    assert!(
        on_board(&state, &bob, &b).await.is_empty(),
        "Off: no assignment, no mention, no move, no comment"
    );

    // Bob watches T2 himself: its moves reach him through Off.
    let t2 = ticket(&state, &b, &ada_c, json!({ "title": "Watched" })).await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t2}/watchers"),
        &bob_c,
        json!({ "watcher": bob_e }),
    )
    .await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t2}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-status"]
    ));

    // Ada (an editor) makes Bob watch T3: involvement, not his choice — Off
    // holds.
    let t3 = ticket(&state, &b, &ada_c, json!({ "title": "Added" })).await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t3}/watchers"),
        &ada_c,
        json!({ "watcher": bob_e }),
    )
    .await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t3}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    settle().await;
    assert_eq!(
        kinds(&on_board(&state, &bob, &b).await),
        ["task-status"],
        "an editor-added watch does not wake a board set to Off"
    );
    // The ticket names who added it, so Bob's panel can say "Added by Ada"
    // and offer "Watch it myself".
    let origin = |full: &Value| full["watcherOrigins"][0]["addedBy"].clone();
    let full = ok(
        &state,
        "GET",
        &format!("/api/tasks/{t3}"),
        &bob_c,
        Value::Null,
    )
    .await;
    assert_eq!(full["watchers"], json!([bob_e]));
    assert_eq!(origin(&full)["id"], json!(ada));
    assert!(
        origin(&full)["name"]
            .as_str()
            .is_some_and(|n| n.starts_with("Ada")),
        "named by the account's name: {full}"
    );
    // Bob presses Watch on it: now it is his.
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t3}/watchers"),
        &bob_c,
        json!({ "watcher": bob_e }),
    )
    .await;
    let added_by: Option<String> =
        sqlx::query_scalar("select added_by from task_watchers where task_id = $1::uuid")
            .bind(&t3)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(added_by.as_deref(), Some(bob.as_str()));
    let full = ok(
        &state,
        "GET",
        &format!("/api/tasks/{t3}"),
        &bob_c,
        Value::Null,
    )
    .await;
    assert_eq!(origin(&full), Value::Null, "his own watch names nobody");
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t3}"),
        &ada_c,
        json!({ "status": "assigned" }),
    )
    .await;
    assert!(eventually!(
        on_board(&state, &bob, &b)
            .await
            .iter()
            .filter(|r| r.kind == "task-status")
            .count()
            == 2
    ));
    // An editor re-adding it never takes it back.
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t3}/watchers"),
        &ada_c,
        json!({ "watcher": bob_e }),
    )
    .await;
    let added_by: Option<String> =
        sqlx::query_scalar("select added_by from task_watchers where task_id = $1::uuid")
            .bind(&t3)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(added_by.as_deref(), Some(bob.as_str()));
}

// ALL: the tickets a person is not in arrive as ONE counting board-activity
// row per board (quiet, like a status move), and the tickets they are in keep
// their own rows. A read row means the next event files a fresh one.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn all_files_one_counting_activity_row_per_board() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, _) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, _, _) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "bn all",
        &[(&bob_e, "viewer"), (&cy_e, "editor")],
    )
    .await;
    set(&state, &b, &bob_c, json!({ "preset": "all" })).await;
    let t1 = ticket(&state, &b, &ada_c, json!({ "title": "One" })).await;
    let t2 = ticket(&state, &b, &ada_c, json!({ "title": "Two" })).await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t1}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t2}/comments"),
        &ada_c,
        json!({ "content": "a note" }),
    )
    .await;
    assert!(eventually!(
        on_board(&state, &bob, &b)
            .await
            .first()
            .is_some_and(|r| r.count == 4)
    ));
    let rows = on_board(&state, &bob, &b).await;
    assert_eq!(kinds(&rows), ["board-activity"], "one row for the board");
    let r = &rows[0];
    assert_eq!(
        r.group_key.as_deref(),
        Some(format!("board:{b}:activity").as_str())
    );
    assert_eq!(r.entity_type.as_deref(), Some("board"));
    assert_eq!(
        r.alert.as_deref(),
        Some("feed"),
        "quiet by default: never louder than a ticket you are in"
    );
    // Cy, with no setting, hears none of it.
    settle().await;
    assert!(on_board(&state, &cy, &b).await.is_empty());

    // Bob reads it; the next event files a fresh row.
    ok(
        &state,
        "PUT",
        "/api/notifications",
        &bob_c,
        json!({ "ids": [r.id] }),
    )
    .await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t2}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    assert!(eventually!(on_board(&state, &bob, &b).await.len() == 2));
    let rows = on_board(&state, &bob, &b).await;
    assert_eq!(rows[1].count, 1);
    assert!(rows[1].body.contains("in progress"), "{:?}", rows[1]);

    // A ticket Bob is in keeps its own per-ticket row.
    let mine = ticket(
        &state,
        &b,
        &ada_c,
        json!({ "title": "Mine", "assignees": [format!("user:{bob}")] }),
    )
    .await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{mine}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await)
            .iter()
            .filter(|k| **k == "task-status" || **k == "task-assigned")
            .count()
            == 2
    ));
    // The unread board row is still at 1: had the new ticket he is assigned
    // (or its move) leaked into it, it would count, not add a row.
    settle().await;
    let activity: Vec<Row> = on_board(&state, &bob, &b)
        .await
        .into_iter()
        .filter(|r| r.kind == "board-activity")
        .collect();
    assert_eq!(activity.len(), 2, "{activity:?}");
    assert_eq!(
        activity[1].count, 1,
        "the ticket he is in never lands in the board row: {:?}",
        activity[1]
    );
}

// ONE ACTION, ONE ROW when a single patch assigns a person and moves the
// ticket: a setting that drops assignments but keeps status hears the move —
// once, as a status row — and no assignment row.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn assigned_and_moved_at_once_is_one_row_by_the_setting() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, _) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "bn assign move", &[(&bob_e, "editor")]).await;
    set(
        &state,
        &b,
        &bob_c,
        json!({ "preset": "custom", "events": ["status"] }),
    )
    .await;
    let t = ticket(&state, &b, &ada_c, json!({ "title": "Both at once" })).await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "status": "in_progress", "assignees": [format!("user:{bob}")] }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-status"]
    ));
    settle().await;
    let rows = on_board(&state, &bob, &b).await;
    assert_eq!(kinds(&rows), ["task-status"], "no assignment row on top");
    assert_eq!(rows[0].count, 1);
}

// A NEW TICKET whose description @mentions someone who hears the whole
// board: the mention row, never a "New ticket" board row on top of it.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_mention_on_a_new_ticket_is_one_row_for_a_whole_board_listener() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, bob_tok) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c, _) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "bn create mention",
        &[(&bob_e, "viewer"), (&cy_e, "viewer")],
    )
    .await;
    set(&state, &b, &bob_c, json!({ "preset": "all" })).await;
    set(&state, &b, &cy_c, json!({ "preset": "all" })).await;
    ticket(
        &state,
        &b,
        &ada_c,
        json!({ "title": "Named", "description": format!("@{bob_tok} over to you") }),
    )
    .await;
    // Cy, named by nobody, hears the new ticket on the board row.
    assert!(eventually!(
        kinds(&on_board(&state, &cy, &b).await) == ["board-activity"]
    ));
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["mention"]
    ));
    settle().await;
    assert_eq!(
        kinds(&on_board(&state, &bob, &b).await),
        ["mention"],
        "the mention is the louder row; no board row on top"
    );
}

// The probe that lets a producer skip reading a whole board, leg by leg, end
// to end: a custom row over the board, then a default of All on a board
// where nobody has a row (talaria_notify::boards::board_wide_possible). The
// board leg runs first: a default of All anywhere answers yes for every
// board, so it would hide a broken board leg — no other test sets one, and
// this one clears its own.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_board_wide_probe_hears_each_leg() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, _) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c, _) = mint(&state, "Cy", "member").await;

    // The board leg: Cy's custom row hears new tickets over the board.
    let b1 = board(&state, &ada_c, "bn probe row", &[(&cy_e, "viewer")]).await;
    set(
        &state,
        &b1,
        &cy_c,
        json!({ "preset": "custom", "events": ["created"], "scope": "board" }),
    )
    .await;
    ticket(&state, &b1, &ada_c, json!({ "title": "Row leg" })).await;
    assert!(eventually!(
        on_board(&state, &cy, &b1)
            .await
            .first()
            .is_some_and(|r| r.kind == "board-activity" && r.body.contains("Row leg"))
    ));

    // The default leg: Bob's default for every board is All, and nobody has a
    // row for this board.
    let b2 = board(&state, &ada_c, "bn probe default", &[(&bob_e, "viewer")]).await;
    ok(
        &state,
        "PATCH",
        "/api/notifications",
        &bob_c,
        json!({ "boards": { "preset": "all" } }),
    )
    .await;
    let rows: i64 =
        sqlx::query_scalar("select count(*) from board_notify_prefs where board_id = $1::uuid")
            .bind(&b2)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(
        rows, 0,
        "no row for the board: only the default can hear it"
    );
    let got = ok(
        &state,
        "GET",
        &format!("/api/boards/{b2}/notify"),
        &bob_c,
        Value::Null,
    )
    .await;
    assert_eq!(
        (got["source"].as_str(), got["effective"]["preset"].as_str()),
        (Some("default"), Some("all"))
    );
    ticket(&state, &b2, &ada_c, json!({ "title": "Default leg" })).await;
    let heard = eventually!(
        on_board(&state, &bob, &b2)
            .await
            .first()
            .is_some_and(|r| r.kind == "board-activity" && r.body.contains("Default leg"))
    );
    // Clear the default before asserting, so a failure here never leaves an
    // All default behind to hide the board leg on the next run.
    ok(
        &state,
        "PATCH",
        "/api/notifications",
        &bob_c,
        json!({ "boards": null }),
    )
    .await;
    assert!(heard, "{:?}", on_board(&state, &bob, &b2).await);
}

// A custom set without @mentions: the comment that @mentions the person is
// heard as a comment (their setting keeps comments), never as nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_dropped_mention_leaves_the_comment_to_the_setting() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, bob_tok) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "bn custom", &[(&bob_e, "editor")]).await;
    set(
        &state,
        &b,
        &bob_c,
        json!({ "preset": "custom", "events": ["comment"] }),
    )
    .await;
    let t = ticket(&state, &b, &bob_c, json!({ "title": "Bob's own" })).await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t}/comments"),
        &ada_c,
        json!({ "content": format!("@{bob_tok} look") }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-comment"]
    ));
    settle().await;
    assert_eq!(kinds(&on_board(&state, &bob, &b).await), ["task-comment"]);
}

// The routes: any board role reads and writes only its own row; nobody else
// can; the checks answer their sentences; DELETE goes back to the default,
// which PATCH /api/notifications sets; the Settings list says where each
// board's setting comes from.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_routes_write_only_the_callers_own_setting() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (cy, cy_e, cy_c, _) = mint(&state, "Cy", "member").await;
    let (_eve, _, eve_c, _) = mint(&state, "Eve", "member").await;
    let b = board(&state, &ada_c, "bn routes", &[(&cy_e, "viewer")]).await;
    let path = format!("/api/boards/{b}/notify");
    // A viewer chooses what they hear.
    let got = set(
        &state,
        &b,
        &cy_c,
        json!({ "preset": "custom", "events": ["created", "status"], "scope": "board" }),
    )
    .await;
    assert_eq!(got["source"], "board");
    assert_eq!(got["effective"]["events"], json!(["created", "status"]));
    assert_eq!(got["effective"]["scope"], "board");
    assert_eq!(got["registry"]["events"].as_array().unwrap().len(), 7);
    // The daily board digest was withdrawn: its cadence is listed, unavailable.
    assert_eq!(
        got["registry"]["cadences"],
        json!([
            { "id": "instant", "label": "Instant", "available": true },
            { "id": "digest", "label": "Daily digest", "available": false },
        ])
    );
    // Ada's own setting is untouched by Cy's.
    let ada_view = ok(&state, "GET", &path, &ada_c, Value::Null).await;
    assert_eq!(ada_view["source"], "builtin");
    let rows: i64 =
        sqlx::query_scalar("select count(*) from board_notify_prefs where board_id = $1::uuid")
            .bind(&b)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(rows, 1, "one row: the caller's");
    // Someone with no role on the board: refused.
    for (method, body) in [
        ("GET", None),
        ("PUT", Some(json!({ "preset": "off" }))),
        ("DELETE", None),
    ] {
        let (status, _) = call_json(&state, method, &path, Some(eve_c.as_str()), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
    }
    // No session: 401.
    let (status, _) = call_json(&state, "GET", &path, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A malformed id is a plain miss.
    let (status, _) = call_json(
        &state,
        "GET",
        "/api/boards/not-a-uuid/notify",
        Some(cy_c.as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The checks.
    for (body, msg) in [
        (
            json!({ "preset": "all", "events": ["comment"] }),
            "events and scope are set only with the custom preset",
        ),
        (
            json!({ "preset": "custom", "events": ["created"] }),
            "new tickets reach you only with scope board — nobody is in a ticket before it exists",
        ),
        (
            json!({ "preset": "all", "cadence": "digest" }),
            "a daily digest isn't available yet",
        ),
    ] {
        let (status, out) = call_json(&state, "PUT", &path, Some(cy_c.as_str()), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(out["error"], msg);
    }
    // Nor is a digest the default for every board: PATCH refuses it in the
    // same words, and stores nothing.
    let (status, out) = call_json(
        &state,
        "PATCH",
        "/api/notifications",
        Some(cy_c.as_str()),
        Some(json!({ "boards": { "preset": "all", "cadence": "digest" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(out["error"], "a daily digest isn't available yet");
    let kept: Option<Value> =
        sqlx::query_scalar("select notify_prefs -> 'boards' from users where id = $1::uuid")
            .bind(&cy)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(kept, None, "a refused default stores nothing");
    // The default for every board, then DELETE back to it.
    let patched = ok(
        &state,
        "PATCH",
        "/api/notifications",
        &cy_c,
        json!({ "boards": { "preset": "mentions" } }),
    )
    .await;
    assert_eq!(patched["boards"]["source"], "default");
    let gone = ok(&state, "DELETE", &path, &cy_c, Value::Null).await;
    assert_eq!(gone["source"], "default");
    assert_eq!(gone["effective"]["preset"], "mentions");
    assert_eq!(gone["stored"], Value::Null);
    let listed = ok(
        &state,
        "GET",
        "/api/notifications/boards",
        &cy_c,
        Value::Null,
    )
    .await;
    assert_eq!(listed["default"]["effective"]["preset"], "mentions");
    // The vocabulary rides with the list, for the default's form.
    assert_eq!(listed["registry"]["presets"][0]["id"], "all");
    let row = listed["boards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(b))
        .expect("the board is listed")
        .clone();
    assert_eq!(row["source"], "default");
    assert_eq!(row["role"], "viewer");
    let fetched = ok(&state, "GET", "/api/notifications", &cy_c, Value::Null).await;
    assert_eq!(fetched["boards"]["effective"]["preset"], "mentions");
    assert!(
        fetched["features"]
            .as_array()
            .unwrap()
            .contains(&json!("board-prefs"))
    );
    assert!(
        !fetched["features"]
            .as_array()
            .unwrap()
            .contains(&json!("board-digest")),
        "the withdrawn board digest is not served"
    );
    // Clearing the default: the built-in again.
    let cleared = ok(
        &state,
        "PATCH",
        "/api/notifications",
        &cy_c,
        json!({ "boards": null }),
    )
    .await;
    assert_eq!(cleared["boards"]["source"], "builtin");
}

/// The statement of ui/src/server/db/pg.ts's MIGRATIONS array that holds
/// `needle`, verbatim: the text every database runs at boot. A statement
/// carries no backtick (check-invariants.mjs refuses one), so the backticks
/// either side of the needle are its own.
fn migration_holding(needle: &str) -> String {
    const PG_TS: &str = include_str!("../../../ui/src/server/db/pg.ts");
    let start = PG_TS
        .find("const MIGRATIONS: string[] = [")
        .expect("pg.ts has its MIGRATIONS array");
    let at = start
        + PG_TS[start..]
            .find(needle)
            .unwrap_or_else(|| panic!("no migration holds {needle:?}"));
    let open = PG_TS[..at].rfind('`').expect("its opening backtick") + 1;
    let close = at + PG_TS[at..].find('`').expect("its closing backtick");
    PG_TS[open..close].to_string()
}

// THE DAILY BOARD DIGEST WAS WITHDRAWN before it reached a release, and a
// testing build that served it may have stored a digest cadence — on a
// board's row, or on the default for every board. Held for a digest nothing
// files, its owner would hear nothing from that board. Two layers make sure
// they hear it as it happens: this binary reads a stored digest as instant
// (shown through the router while the stored value still says digest), and
// the migration sets each one back to instant, everything else kept (its own
// statements, read out of pg.ts and run here over rows seeded the way that
// build stored them). The digest's ledger is gone from the migrated schema.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_daily_digest_stored_before_the_withdrawal_is_heard_instantly() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, _) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c, _) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "bn digest withdrawn",
        &[(&bob_e, "editor"), (&cy_e, "viewer")],
    )
    .await;
    // Bob: Participating on this board, as a digest, with Email on. Cy: All
    // on a digest as the default for every board. Neither route accepts that
    // any more, so they are written the way the digest's build stored them.
    sqlx::query(
        "insert into board_notify_prefs (user_id, board_id, preset, cadence, email) \
         values ($1::uuid, $2::uuid, 'participating', 'digest', true)",
    )
    .bind(&bob)
    .bind(&b)
    .execute(&state.pg)
    .await
    .unwrap();
    sqlx::query(
        "update users set notify_prefs = notify_prefs || \
           '{\"boards\": {\"preset\": \"all\", \"cadence\": \"digest\"}}'::jsonb \
         where id = $1::uuid",
    )
    .bind(&cy)
    .execute(&state.pg)
    .await
    .unwrap();

    // This binary hears both as instant, the rest of each setting kept.
    let path = format!("/api/boards/{b}/notify");
    let bob_view = ok(&state, "GET", &path, &bob_c, Value::Null).await;
    assert_eq!(bob_view["source"], "board");
    assert_eq!(bob_view["effective"]["preset"], "participating");
    assert_eq!(bob_view["effective"]["cadence"], "instant");
    assert_eq!(bob_view["effective"]["channels"]["email"], json!(true));
    let cy_view = ok(&state, "GET", &path, &cy_c, Value::Null).await;
    assert_eq!(cy_view["source"], "default");
    assert_eq!(cy_view["effective"]["preset"], "all");
    assert_eq!(cy_view["effective"]["cadence"], "instant");
    // So a ticket's news reaches them as it happens: Bob his assignment and
    // the move of a ticket he is in (a digest held the move), Cy the counting
    // board row (a digest held both events).
    let t = ticket(
        &state,
        &b,
        &ada_c,
        json!({ "title": "Withdrawn digest", "assignees": [format!("user:{bob}")] }),
    )
    .await;
    // The creation fan-out is detached: let it land before the move's.
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-assigned"]
    ));
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["task-assigned", "task-status"]
    ));
    assert!(eventually!(
        on_board(&state, &cy, &b)
            .await
            .first()
            .is_some_and(|r| r.kind == "board-activity" && r.count == 2)
    ));
    // The stored values still say digest: the read above was this binary's.
    let raw_row: String = sqlx::query_scalar(
        "select cadence from board_notify_prefs where user_id = $1::uuid and board_id = $2::uuid",
    )
    .bind(&bob)
    .bind(&b)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(raw_row, "digest");

    // The migration's own statements, over those rows.
    for needle in [
        "update board_notify_prefs set cadence = 'instant'",
        "jsonb_set(notify_prefs, '{boards,cadence}'",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(migration_holding(needle)))
            .execute(&state.pg)
            .await
            .unwrap();
    }
    let (preset, cadence, email): (String, String, Option<bool>) = sqlx::query_as(
        "select preset, cadence, email from board_notify_prefs \
         where user_id = $1::uuid and board_id = $2::uuid",
    )
    .bind(&bob)
    .bind(&b)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(
        (preset.as_str(), cadence.as_str(), email),
        ("participating", "instant", Some(true))
    );
    let prefs: Value = sqlx::query_scalar("select notify_prefs from users where id = $1::uuid")
        .bind(&cy)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    assert_eq!(
        prefs["boards"],
        json!({ "preset": "all", "cadence": "instant" })
    );
    // The approvals mail's own switch is a different key, and untouched.
    assert_eq!(prefs["digest"], "off");
    // And the routes answer as before.
    let bob_after = ok(&state, "GET", &path, &bob_c, Value::Null).await;
    assert_eq!(bob_after["effective"], bob_view["effective"]);

    // The digest's ledger is not in the migrated schema.
    let ledger: Option<String> =
        sqlx::query_scalar("select to_regclass('public.board_digest_state')::text")
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(ledger, None);
}

// Leaving a board keeps the setting (dormant: nothing reaches someone who
// cannot see the board), and rejoining applies it again.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_setting_survives_leaving_and_rejoining() {
    let state = live_app_state().await;
    let (_ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, _) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "bn rejoin", &[(&bob_e, "editor")]).await;
    set(&state, &b, &bob_c, json!({ "preset": "all" })).await;
    ok(
        &state,
        "DELETE",
        &format!("/api/boards/{b}/members"),
        &ada_c,
        json!({ "userId": bob }),
    )
    .await;
    ticket(&state, &b, &ada_c, json!({ "title": "While away" })).await;
    settle().await;
    assert!(
        on_board(&state, &bob, &b).await.is_empty(),
        "no access, nothing"
    );
    let kept: i64 = sqlx::query_scalar(
        "select count(*) from board_notify_prefs where board_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(&b)
    .bind(&bob)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(kept, 1, "kept while away");
    ok(
        &state,
        "POST",
        &format!("/api/boards/{b}/members"),
        &ada_c,
        json!({ "email": bob_e, "role": "editor" }),
    )
    .await;
    let back = ok(
        &state,
        "GET",
        &format!("/api/boards/{b}/notify"),
        &bob_c,
        Value::Null,
    )
    .await;
    assert_eq!(
        (
            back["source"].as_str(),
            back["effective"]["preset"].as_str()
        ),
        (Some("board"), Some("all"))
    );
    ticket(&state, &b, &ada_c, json!({ "title": "Back again" })).await;
    assert!(eventually!(
        kinds(&on_board(&state, &bob, &b).await) == ["board-activity"]
    ));
}

// A team's members see its boards with no member row of their own: they can
// set a board and the board's whole audience includes them. No ticket
// mention door has them on its roster, so a comment that @mentions one is no
// reason to leave them out of the comment's fan-out: they hear it on their
// board row, never nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_team_member_sets_and_obeys_a_board_setting() {
    let state = live_app_state().await;
    let (ada, _, ada_c, _) = mint(&state, "Ada", "admin").await;
    let (dee, _, dee_c, dee_tok) = mint(&state, "Dee", "member").await;
    let team: String = sqlx::query_scalar(
        "insert into teams (name, created_by) values ($1, $2::uuid) returning id::text",
    )
    .bind(format!("bn team {}", &ada[..8]))
    .bind(&ada)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    for (user, role) in [(&ada, "owner"), (&dee, "member")] {
        sqlx::query(
            "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, $3)",
        )
        .bind(&team)
        .bind(user)
        .bind(role)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    let b = id_of(
        &ok(
            &state,
            "POST",
            "/api/boards",
            &ada_c,
            json!({ "name": "bn team board", "teamId": team }),
        )
        .await,
        "board",
    );
    let direct: i64 = sqlx::query_scalar(
        "select count(*) from board_members where board_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(&b)
    .bind(&dee)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(direct, 0, "Dee is on the board through the team alone");
    set(&state, &b, &dee_c, json!({ "preset": "all" })).await;
    let t = ticket(&state, &b, &ada_c, json!({ "title": "For the team" })).await;
    assert!(eventually!(
        on_board(&state, &dee, &b)
            .await
            .first()
            .is_some_and(|r| r.kind == "board-activity" && r.body.contains("For the team"))
    ));
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t}/comments"),
        &ada_c,
        json!({ "content": format!("@{dee_tok} have a look") }),
    )
    .await;
    assert!(eventually!(
        on_board(&state, &dee, &b)
            .await
            .first()
            .is_some_and(|r| r.count == 2)
    ));
    settle().await;
    let rows = on_board(&state, &dee, &b).await;
    assert_eq!(
        kinds(&rows),
        ["board-activity"],
        "no mention row (no door has a team-leg member on its roster), and the comment counted"
    );
    assert!(rows[0].body.contains("commented"), "{:?}", rows[0]);
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Delivered {
    kind: String,
    alert: Option<String>,
    desktop_override: Option<bool>,
    push_override: Option<bool>,
    email_override: Option<bool>,
    /// The write armed the missed-activity mail (`email_due_at` set).
    mail_armed: bool,
}

/// How each row one person holds about this board was filed: its kind, the
/// alert the write decided, the channel override it was filed with, and
/// whether it armed the mail.
async fn delivered(state: &AppState, user: &str, board: &str) -> Vec<Delivered> {
    sqlx::query_as(
        "select kind, alert, desktop_override, push_override, email_override, \
                (email_due_at is not null) as mail_armed \
         from notifications \
         where user_id = $1::uuid and (href like $2 or href = $3 \
               or entity_id in (select id::text from tasks where board_id = $4::uuid)) \
         order by created_at, seq",
    )
    .bind(user)
    .bind(format!("/boards/{board}/%"))
    .bind(format!("/boards/{board}"))
    .bind(board)
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

/// The writer reads the instance's delivery switches through a 10 s
/// in-process cache (talaria-notify `mail::delivery_switches`), and the other
/// tests of this module write rows meanwhile: after a flip, wait it out.
const SWITCH_CACHE: Duration = Duration::from_millis(10_500);

/// Runs `body` with the instance's notification mail switched on
/// (`app_settings.notify_delivery.emailEnabled`, the rest of the row kept),
/// then puts the row back as it was — after a failure too. No batcher runs
/// in this binary, so nothing is ever sent: the switch only lets the write
/// arm `email_due_at`. Every other person in this module has every class
/// in-app (`IN_APP`), so no other test's row arms mail meanwhile.
async fn with_mail_on<F: std::future::Future<Output = ()>>(state: &AppState, body: F) {
    use futures_util::FutureExt as _;
    let saved: Option<Value> =
        sqlx::query_scalar("select value from app_settings where key = 'notify_delivery'")
            .fetch_optional(&state.pg)
            .await
            .unwrap();
    let mut on = saved
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    on.insert("emailEnabled".into(), Value::Bool(true));
    put_delivery(state, &Value::Object(on)).await;
    tokio::time::sleep(SWITCH_CACHE).await;
    let outcome = std::panic::AssertUnwindSafe(body).catch_unwind().await;
    match saved {
        Some(v) => put_delivery(state, &v).await,
        None => {
            sqlx::query("delete from app_settings where key = 'notify_delivery'")
                .execute(&state.pg)
                .await
                .unwrap();
        }
    }
    if let Err(p) = outcome {
        std::panic::resume_unwind(p);
    }
}

async fn put_delivery(state: &AppState, v: &Value) {
    sqlx::query(
        "insert into app_settings (key, value) values ('notify_delivery', $1) \
         on conflict (key) do update set value = excluded.value, updated_at = now()",
    )
    .bind(v)
    .execute(&state.pg)
    .await
    .unwrap();
}

// A BOARD'S OWN CHANNELS (part 2): a person's setting for a board can say how
// its rows reach them — Alerts (the banner and the push, together) and Email.
// Bob keeps his rows off the screen and in the mail; Dee, who hears the whole
// board, has its quiet activity row pop up; Eve set the board to Off with
// mail only and watches one ticket herself, so the rows that watch carries
// through Off come by mail too. Each row is filed with the override its own
// recipient chose, decided at fan-out and stored on the row beside the
// delivery it shaped (the alert and the armed mail here; the deferred push
// and the batcher read the stored override at send time). Bob's and Eve's
// matrix mails nothing — every class is in-app — so the armed mail is the
// board's doing. Cy, with no setting, gets exactly the matrix — another
// person's choice changes nothing she receives. In-app is not a choice.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_boards_own_channels_shape_that_persons_rows_only() {
    let state = live_app_state().await;
    with_mail_on(&state, boards_own_channels(&state)).await;
}

async fn boards_own_channels(state: &AppState) {
    let (ada, _, ada_c, _) = mint(state, "Ada", "admin").await;
    let (bob, bob_e, bob_c, bob_tok) = mint(state, "Bob", "member").await;
    let (cy, cy_e, _, _) = mint(state, "Cy", "member").await;
    let (dee, dee_e, dee_c, _) = mint(state, "Dee", "member").await;
    let (eve, eve_e, eve_c, _) = mint(state, "Eve", "member").await;
    let b = board(
        state,
        &ada_c,
        "bn channels",
        &[
            (&bob_e, "editor"),
            (&cy_e, "editor"),
            (&dee_e, "viewer"),
            (&eve_e, "viewer"),
        ],
    )
    .await;
    let got = set(
        state,
        &b,
        &bob_c,
        json!({ "preset": "participating", "channels": { "alerts": false, "email": true } }),
    )
    .await;
    assert_eq!(
        got["effective"]["channels"],
        json!({ "alerts": false, "email": true })
    );
    assert_eq!(
        got["stored"],
        json!({ "preset": "participating", "cadence": "instant",
                "channels": { "alerts": false, "email": true } })
    );
    set(
        state,
        &b,
        &dee_c,
        json!({ "preset": "all", "channels": { "alerts": true } }),
    )
    .await;
    set(
        state,
        &b,
        &eve_c,
        json!({ "preset": "off", "channels": { "alerts": false, "email": true } }),
    )
    .await;
    // In-app is not a choice, as in the matrix.
    let (status, out) = call_json(
        state,
        "PUT",
        &format!("/api/boards/{b}/notify"),
        Some(&dee_c),
        Some(json!({ "preset": "all", "channels": { "inApp": false } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert_eq!(out["error"], "In-app is always on");

    // Assigned to Bob and Cy; Eve watches it herself; moved, then a comment
    // @mentioning Bob.
    let t = ticket(
        state,
        &b,
        &ada_c,
        json!({ "title": "Channels", "assignees": [format!("user:{bob}"), format!("user:{cy}")] }),
    )
    .await;
    ok(
        state,
        "POST",
        &format!("/api/tasks/{t}/watchers"),
        &eve_c,
        json!({ "watcher": eve_e }),
    )
    .await;
    ok(
        state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    ok(
        state,
        "POST",
        &format!("/api/tasks/{t}/comments"),
        &ada_c,
        json!({ "content": format!("@{bob_tok} over to you") }),
    )
    .await;
    assert!(eventually!(
        kind_set(&on_board(state, &bob, &b).await) == ["mention", "task-assigned", "task-status"]
    ));
    assert!(eventually!(
        kind_set(&on_board(state, &cy, &b).await)
            == ["task-assigned", "task-comment", "task-status"]
    ));
    assert!(eventually!(
        on_board(state, &dee, &b)
            .await
            .first()
            .is_some_and(|r| r.kind == "board-activity" && r.count == 3)
    ));
    assert!(eventually!(
        kind_set(&on_board(state, &eve, &b).await) == ["task-comment", "task-status"]
    ));
    settle().await;

    // Bob: every row — the assignment, the move and the @mention door's —
    // filed with his board's channels: off the screen (feed, push off), and
    // the mail armed although his matrix mails none of these classes.
    let rows = delivered(state, &bob, &b).await;
    assert_eq!(rows.len(), 3, "{rows:?}");
    for r in &rows {
        assert_eq!(
            (
                r.desktop_override,
                r.push_override,
                r.email_override,
                r.alert.as_deref(),
                r.mail_armed
            ),
            (Some(false), Some(false), Some(true), Some("feed"), true),
            "{r:?}"
        );
    }
    // Cy: the matrix, untouched — every row banners (her classes are in-app),
    // none carries an override, and none is mailed.
    let rows = delivered(state, &cy, &b).await;
    assert_eq!(rows.len(), 3, "{rows:?}");
    for r in &rows {
        assert_eq!(
            (
                r.desktop_override,
                r.push_override,
                r.email_override,
                r.alert.as_deref(),
                r.mail_armed
            ),
            (None, None, None, Some("banner"), false),
            "{r:?}"
        );
    }
    // Dee: the board-activity row is quiet by default; her board's Alerts
    // made it pop up (and push), and left the mail to the matrix.
    let rows = delivered(state, &dee, &b).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(
        (
            r.kind.as_str(),
            r.desktop_override,
            r.push_override,
            r.email_override,
            r.alert.as_deref(),
            r.mail_armed
        ),
        (
            "board-activity",
            Some(true),
            Some(true),
            None,
            Some("banner"),
            false
        ),
        "{r:?}"
    );
    // Eve: Off holds everything but her own watch, and what the watch carries
    // comes on her board's channels — mail, not the screen.
    let rows = delivered(state, &eve, &b).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert_eq!(
            (
                r.desktop_override,
                r.push_override,
                r.email_override,
                r.alert.as_deref(),
                r.mail_armed
            ),
            (Some(false), Some(false), Some(true), Some("feed"), true),
            "{r:?}"
        );
    }
    assert!(
        delivered(state, &ada, &b).await.is_empty(),
        "never the actor"
    );
    // The feature is served.
    let fetched = ok(state, "GET", "/api/notifications", &bob_c, Value::Null).await;
    assert!(
        fetched["features"]
            .as_array()
            .unwrap()
            .contains(&json!("board-channels"))
    );
    // A PUT is the whole setting: one without channels gives the matrix back.
    let back = set(state, &b, &bob_c, json!({ "preset": "participating" })).await;
    assert_eq!(
        back["effective"]["channels"],
        json!({ "alerts": null, "email": null })
    );
    assert_eq!(
        back["stored"],
        json!({ "preset": "participating", "cadence": "instant" })
    );
}

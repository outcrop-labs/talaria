// Live-DB proof for the due-date reminders (talaria_tasks::due, the
// `task-due` job; docs/notifications/producers.md "Due dates"): opt-in — with
// nobody's setting naming due dates nobody is told, and a watch turns on
// nothing new, but the thresholds are marked all the same; a Custom set over
// the tickets you are in reminds you once per threshold — "due soon", then
// "overdue" into the same row — and moving the due date re-arms it; a default
// for every board and a board row of All opt in too; closed, archived and
// long-overdue tickets are not news; a whole-board setting puts the rest of
// the board's reminders on its one activity row; someone who left the board
// hears nothing.
// The settings go in through the REAL router with minted sessions; the scan
// is the job's own pass (`talaria_tasks::due::tick`), run in place of waiting
// for the scheduler, one at a time as the job's lease runs it. A pass reads
// every ticket in the band, other tests' included, so each test reads only
// its own people's rows, and a ticket whose setup must finish before any pass
// sees it (closed, archived, its assignee gone) gets its due date last.
// House rule: #[ignore]d; ci.yml's api job runs them with --ignored (its
// live-DB steps).
//
//   source ui/.env && cargo test -p talaria-api --test it task_due_live:: -- --ignored

use std::sync::LazyLock;

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// One throwaway person: every class in-app (nothing is ever mailed), a
/// minted session. (id, email, cookie).
async fn mint(state: &AppState, first: &str, role: &str) -> (String, String, String) {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let email = format!(
        "td-{}-{}@task-due.invalid",
        first.to_lowercase(),
        &tag[..12]
    );
    let sub = format!("task-due-live:{tag}");
    let name = format!("{first}{} Td", &tag[..6]);
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

async fn ok(state: &AppState, method: &str, path: &str, cookie: &str, body: Value) -> Value {
    let body = (!body.is_null()).then_some(body);
    let (status, out) = call_json(state, method, path, Some(cookie), body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path} → {out}");
    out
}

async fn board(state: &AppState, owner: &str, name: &str, members: &[(&str, &str)]) -> String {
    let out = ok(state, "POST", "/api/boards", owner, json!({ "name": name })).await;
    let b = out["board"]["id"].as_str().expect("a board id").to_string();
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

/// A ticket with no due date yet, assigned to `assignee`.
async fn ticket(
    state: &AppState,
    board: &str,
    cookie: &str,
    title: &str,
    assignee: &str,
) -> String {
    let out = ok(
        state,
        "POST",
        &format!("/api/boards/{board}/tasks"),
        cookie,
        json!({ "title": title, "assignees": [format!("user:{assignee}")] }),
    )
    .await;
    out["task"]["id"].as_str().expect("a task id").to_string()
}

/// Give a ticket its due date, `minutes` from now — last, once its setup is
/// done, so no pass can see it half set up.
async fn due_in(state: &AppState, task: &str, minutes: i32) {
    sqlx::query(
        "update tasks set due_date = now() + $2::int * interval '1 minute' where id = $1::uuid",
    )
    .bind(task)
    .bind(minutes)
    .execute(&state.pg)
    .await
    .unwrap();
}

/// A ticket due `minutes` from now (negative: ago), assigned to `assignee`,
/// made with its due date through the route.
async fn ticket_due(
    state: &AppState,
    board: &str,
    cookie: &str,
    title: &str,
    assignee: &str,
    minutes: i32,
) -> String {
    let due = iso_from_now(state, minutes).await;
    let out = ok(
        state,
        "POST",
        &format!("/api/boards/{board}/tasks"),
        cookie,
        json!({ "title": title, "assignees": [format!("user:{assignee}")], "dueDate": due }),
    )
    .await;
    out["task"]["id"].as_str().expect("a task id").to_string()
}

/// The database's clock, `minutes` from now, as the routes take a datetime.
async fn iso_from_now(state: &AppState, minutes: i32) -> String {
    sqlx::query_scalar(
        "select to_char((now() + $1::int * interval '1 minute') at time zone 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')",
    )
    .bind(minutes)
    .fetch_one(&state.pg)
    .await
    .unwrap()
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

/// One pass at a time, as the job's lease runs them.
static PASS: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// One scan, as the scheduler would run it.
async fn pass(state: &AppState) {
    let _one = PASS.lock().await;
    let notify = talaria_notify::NotifyDeps::publishing(state.pg.clone(), None);
    talaria_tasks::due::tick(&notify)
        .await
        .expect("the due scan runs");
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    title: String,
    body: String,
    count: i32,
    group_key: Option<String>,
    entity_type: Option<String>,
    actor_kind: Option<String>,
}

/// One person's `task-due` rows about this ticket, oldest first.
async fn due_rows(state: &AppState, user: &str, task: &str) -> Vec<Row> {
    sqlx::query_as(
        "select title, body, count, group_key, entity_type, actor_kind from notifications \
         where user_id = $1::uuid and kind = 'task-due' and entity_id = $2 \
         order by created_at, seq",
    )
    .bind(user)
    .bind(task)
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

/// One person's rows about this board as a whole (`board-activity`).
async fn activity_rows(state: &AppState, user: &str, board: &str) -> Vec<Row> {
    sqlx::query_as(
        "select title, body, count, group_key, entity_type, actor_kind from notifications \
         where user_id = $1::uuid and kind = 'board-activity' and entity_id = $2 \
         order by created_at, seq",
    )
    .bind(user)
    .bind(board)
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

/// Every `task-due` row anyone holds about this ticket.
async fn anyone_told(state: &AppState, task: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from notifications where kind = 'task-due' and entity_id = $1",
    )
    .bind(task)
    .fetch_one(&state.pg)
    .await
    .unwrap()
}

/// (due soon fired for this due date, overdue fired for this due date).
async fn marks(state: &AppState, task: &str) -> (bool, bool) {
    sqlx::query_as(
        "select due_soon_for is not distinct from due_date and due_soon_for is not null, \
                overdue_for is not distinct from due_date and overdue_for is not null \
         from tasks where id = $1::uuid",
    )
    .bind(task)
    .fetch_one(&state.pg)
    .await
    .unwrap()
}

async fn ticket_ref(state: &AppState, task: &str) -> String {
    sqlx::query_scalar(
        "select coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no \
         from tasks t join boards b on b.id = t.board_id where t.id = $1::uuid",
    )
    .bind(task)
    .fetch_one(&state.pg)
    .await
    .unwrap()
}

// EXISTING USERS SEE NO CHANGE. Nobody on the board chose due dates: the
// assignee, the creator and a viewer all hear nothing, and the thresholds
// are marked all the same, so choosing due dates later never replays one
// that already passed. Then a board where the creator chose them before the
// pass, so the full read runs (the probe is true): she hears it, while the
// assignee and a viewer watching the ticket herself, neither with a setting,
// hear nothing — a watch turns on nothing new.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn nobody_hears_a_due_date_until_they_opt_in() {
    let state = live_app_state().await;
    let (ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, _) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "td parity",
        &[(&bob_e, "editor"), (&cy_e, "viewer")],
    )
    .await;
    let soon = ticket_due(&state, &b, &ada_c, "Soon", &bob, 60).await;
    let late = ticket_due(&state, &b, &ada_c, "Late", &bob, -5).await;
    pass(&state).await;
    assert_eq!(anyone_told(&state, &soon).await, 0, "nobody opted in");
    assert_eq!(anyone_told(&state, &late).await, 0, "nobody opted in");
    assert!(activity_rows(&state, &cy, &b).await.is_empty());
    assert_eq!(
        marks(&state, &soon).await,
        (true, false),
        "due soon decided"
    );
    assert_eq!(marks(&state, &late).await, (false, true), "overdue decided");
    // Opting in afterwards does not replay a threshold already passed.
    set(
        &state,
        &b,
        &ada_c,
        json!({ "preset": "custom", "events": ["due"] }),
    )
    .await;
    pass(&state).await;
    assert_eq!(anyone_told(&state, &soon).await, 0);
    assert_eq!(anyone_told(&state, &late).await, 0);

    // Ada chose due dates on this board before its ticket comes due, so the
    // pass reads the ticket's audience and every setting in it. Cy watches
    // the ticket herself (a deliberate watch) and has no setting; Bob is
    // assigned and has none.
    let b2 = board(
        &state,
        &ada_c,
        "td watch",
        &[(&bob_e, "editor"), (&cy_e, "viewer")],
    )
    .await;
    set(
        &state,
        &b2,
        &ada_c,
        json!({ "preset": "custom", "events": ["due"] }),
    )
    .await;
    let watched = ticket(&state, &b2, &ada_c, "Watched", &bob).await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{watched}/watchers"),
        &cy_c,
        json!({ "watcher": cy_e }),
    )
    .await;
    due_in(&state, &watched, 60).await;
    pass(&state).await;
    let rows = due_rows(&state, &ada, &watched).await;
    assert_eq!(rows.len(), 1, "the creator chose due dates: {rows:?}");
    assert!(
        due_rows(&state, &cy, &watched).await.is_empty(),
        "a watch turns on nothing new"
    );
    assert!(activity_rows(&state, &cy, &b2).await.is_empty());
    assert!(
        due_rows(&state, &bob, &watched).await.is_empty(),
        "assigned, no setting"
    );
    assert_eq!(anyone_told(&state, &watched).await, 1);
}

// The other ways in: a default for every board, with no row for this board
// (the probe's default leg reads users.notify_prefs -> 'boards'), and a
// board row of preset All. Each person is the only one on their board with
// anything stored, so their own leg is what opens the full read.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_default_or_all_opts_in_as_well() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "td default", &[(&bob_e, "editor")]).await;
    ok(
        &state,
        "PATCH",
        "/api/notifications",
        &bob_c,
        json!({ "boards": { "preset": "custom", "events": ["due"] } }),
    )
    .await;
    let rows: i64 =
        sqlx::query_scalar("select count(*) from board_notify_prefs where board_id = $1::uuid")
            .bind(&b)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(rows, 0, "no row for the board: only the default opts in");
    let t = ticket(&state, &b, &ada_c, "By default", &bob).await;
    due_in(&state, &t, 60).await;
    pass(&state).await;
    let heard = due_rows(&state, &bob, &t).await;
    // Clear the default before asserting, so a failure here never leaves a
    // due default behind to open the probe for every other board.
    ok(
        &state,
        "PATCH",
        "/api/notifications",
        &bob_c,
        json!({ "boards": null }),
    )
    .await;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert!(heard[0].title.ends_with(": due soon"), "{heard:?}");

    // A board row of All: the probe's preset leg (All stores no events).
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let b2 = board(&state, &ada_c, "td all", &[(&cy_e, "editor")]).await;
    set(&state, &b2, &cy_c, json!({ "preset": "all" })).await;
    let t2 = ticket(&state, &b2, &ada_c, "All of it", &cy).await;
    due_in(&state, &t2, 60).await;
    pass(&state).await;
    let rows = due_rows(&state, &cy, &t2).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
}

// A Custom set over the tickets you are in: one row per ticket, "due soon"
// then "overdue" into it (re-alerting), each once per due date; moving the
// due date re-arms it. Nobody else on the board hears it, and closed,
// archived and long-overdue tickets are not news.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_custom_set_reminds_me_once_per_threshold() {
    let state = live_app_state().await;
    let (ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, _) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "td custom",
        &[(&bob_e, "editor"), (&cy_e, "viewer")],
    )
    .await;
    // The registry serves due dates as choosable, and the feature is on.
    let got = set(
        &state,
        &b,
        &bob_c,
        json!({ "preset": "custom", "events": ["due"] }),
    )
    .await;
    assert_eq!(got["effective"]["events"], json!(["due"]));
    assert_eq!(got["effective"]["scope"], "mine");
    let due_event = got["registry"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "due")
        .cloned()
        .expect("due is listed");
    assert_eq!(due_event["available"], true);
    let features = ok(&state, "GET", "/api/notifications", &bob_c, Value::Null).await;
    assert!(
        features["features"]
            .as_array()
            .unwrap()
            .contains(&json!("task-due"))
    );

    let t = ticket_due(&state, &b, &ada_c, "Ship it", &bob, 60).await;
    let r = ticket_ref(&state, &t).await;
    pass(&state).await;
    let rows = due_rows(&state, &bob, &t).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].title, format!("{r}: due soon"));
    assert_eq!(rows[0].body, "Ship it");
    assert_eq!(rows[0].count, 1);
    assert_eq!(
        rows[0].group_key.as_deref(),
        Some(format!("task:{t}:due").as_str())
    );
    assert_eq!(rows[0].entity_type.as_deref(), Some("task"));
    assert_eq!(rows[0].actor_kind.as_deref(), Some("system"));
    // Once per threshold: a second pass files nothing.
    pass(&state).await;
    let rows = due_rows(&state, &bob, &t).await;
    assert_eq!((rows.len(), rows[0].count), (1, 1), "{rows:?}");
    // Nobody else: the creator and a viewer have no setting naming due.
    assert!(due_rows(&state, &ada, &t).await.is_empty());
    assert!(due_rows(&state, &cy, &t).await.is_empty());
    assert!(activity_rows(&state, &cy, &b).await.is_empty());

    // It passes: "overdue" takes over the unread row and re-alerts it.
    let past = iso_from_now(&state, -1).await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "dueDate": past }),
    )
    .await;
    pass(&state).await;
    let rows = due_rows(&state, &bob, &t).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].title, format!("{r}: overdue"));
    assert_eq!(rows[0].count, 2);
    pass(&state).await;
    assert_eq!(due_rows(&state, &bob, &t).await[0].count, 2, "once");

    // Moved out again: a new due date, owed its own "due soon".
    let later = iso_from_now(&state, 120).await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{t}"),
        &ada_c,
        json!({ "dueDate": later }),
    )
    .await;
    pass(&state).await;
    let rows = due_rows(&state, &bob, &t).await;
    assert_eq!(rows[0].title, format!("{r}: due soon"));
    assert_eq!(rows[0].count, 3);

    // Not news: a closed ticket (and it stays unmarked, so reopening it in
    // the band still reminds), an archived one, a long-overdue one.
    let done = ticket(&state, &b, &ada_c, "Closed", &bob).await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{done}"),
        &ada_c,
        json!({ "status": "done" }),
    )
    .await;
    due_in(&state, &done, 60).await;
    let archived = ticket(&state, &b, &ada_c, "Archived", &bob).await;
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{archived}"),
        &ada_c,
        json!({ "archived": true }),
    )
    .await;
    due_in(&state, &archived, 60).await;
    let stale = ticket_due(&state, &b, &ada_c, "Stale", &bob, -3 * 24 * 60).await;
    pass(&state).await;
    for (task, what) in [
        (&done, "closed"),
        (&archived, "archived"),
        (&stale, "long overdue"),
    ] {
        assert!(due_rows(&state, &bob, task).await.is_empty(), "{what}");
    }
    assert_eq!(
        marks(&state, &done).await,
        (false, false),
        "closed: unmarked"
    );
    ok(
        &state,
        "PUT",
        &format!("/api/tasks/{done}"),
        &ada_c,
        json!({ "status": "in_progress" }),
    )
    .await;
    pass(&state).await;
    assert_eq!(due_rows(&state, &bob, &done).await.len(), 1, "reopened");
}

// A whole-board listener: the tickets they are in remind them on their own
// rows, and the rest of the board's reminders land on its one counting
// activity row (the same split All makes, over due dates alone here, so no
// other event shares the row). Someone who opted in and then left the board
// hears nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_whole_board_listener_gets_the_rest_on_its_activity_row() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, _) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let b = board(
        &state,
        &ada_c,
        "td board",
        &[(&bob_e, "editor"), (&cy_e, "viewer")],
    )
    .await;
    set(
        &state,
        &b,
        &cy_c,
        json!({ "preset": "custom", "events": ["due"], "scope": "board" }),
    )
    .await;
    let mine = ticket_due(&state, &b, &ada_c, "Cy's own", &cy, 60).await;
    let theirs = ticket_due(&state, &b, &ada_c, "Bob's", &bob, -5).await;
    pass(&state).await;
    // Her own ticket: its own row.
    let own = due_rows(&state, &cy, &mine).await;
    assert_eq!(own.len(), 1, "{own:?}");
    assert!(own[0].title.ends_with(": due soon"), "{own:?}");
    // Bob's ticket: on the board's activity row, never a per-ticket row.
    assert!(due_rows(&state, &cy, &theirs).await.is_empty());
    let r = ticket_ref(&state, &theirs).await;
    let activity = activity_rows(&state, &cy, &b).await;
    assert_eq!(activity.len(), 1, "{activity:?}");
    assert_eq!(activity[0].body, format!("{r}: overdue"));
    assert_eq!(activity[0].count, 1);
    assert_eq!(
        activity[0].group_key.as_deref(),
        Some(format!("board:{b}:activity").as_str())
    );
    // Bob chose nothing: nothing, his own ticket included.
    assert!(due_rows(&state, &bob, &theirs).await.is_empty());

    // Dee opts in, is assigned, and leaves the board before the due date
    // comes near: her setting is dormant, and nothing reaches her.
    let (dee, dee_e, dee_c) = mint(&state, "Dee", "member").await;
    let b2 = board(&state, &ada_c, "td left", &[(&dee_e, "editor")]).await;
    set(
        &state,
        &b2,
        &dee_c,
        json!({ "preset": "custom", "events": ["due"] }),
    )
    .await;
    let gone = ticket(&state, &b2, &ada_c, "Dee's", &dee).await;
    ok(
        &state,
        "DELETE",
        &format!("/api/boards/{b2}/members"),
        &ada_c,
        json!({ "userId": dee }),
    )
    .await;
    due_in(&state, &gone, 60).await;
    pass(&state).await;
    assert!(due_rows(&state, &dee, &gone).await.is_empty());
    assert!(activity_rows(&state, &dee, &b2).await.is_empty());
    assert_eq!(marks(&state, &gone).await, (true, false), "still decided");
}

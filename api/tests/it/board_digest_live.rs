// Live-DB proof for the daily board digest (talaria_digest::boards,
// docs/notifications/producers.md "Each board"): a board a person set to a
// daily digest files nothing as it happens, and once a day, at the daily
// digest's hour in their own zone, one quiet row per board rolling up what
// the setting covers — new tickets, moves, completions, comments — over the
// stretch since their last digest, minus their own actions and a tracker
// sync's writes; plus one "Your boards" mail when Board activity's Email is
// on. A second tick the same day files nothing and mails nothing; a mail
// that failed (or that the instance switch held) is sent on a later tick
// without filing the rows again; tomorrow's digest counts only what came
// after today's, in a row of its own. Only the boards the person can still
// see, only the ones their setting puts on a digest, never the moves of a
// ticket they watch themselves (those reached them as they happened), a
// ticket moved here only from its arrival, and due dates only as thresholds
// crossed. The ledger's claims: one winner of two at once; a day still
// filing is never mailed, and a claim a dead pass left is taken over and
// finishes the day without filing a row twice.
// Events go through the REAL router with minted sessions; the pass is called
// directly with the person's clock put in the send window and a recording
// mail door, for this module's people only (`run_board_digest_for`), so it
// never touches another test's. The claims and the counts query are reached
// through `talaria_digest::boards::seam`. House rule: #[ignore]d; ci.yml's
// api job runs them with --ignored (its live-DB steps).
//
//   source ui/.env && cargo test -p talaria-api --test it board_digest_live:: -- --ignored

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::state::AppState;
use talaria_digest::Recipient;
use talaria_digest::boards::{
    BoardDigest, BoardDigestDeps, BoardDigestRun, BoardMail, Counts, Raw, SendBoardMail, Tally,
    run_board_digest_for, seam,
};
use talaria_notify::{GatedSendResult, NotifyDeps};

use crate::support::{call_json, live_app_state};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// One throwaway person, every class in-app, a minted session: (id, email,
/// cookie).
async fn mint(state: &AppState, first: &str, role: &str) -> (String, String, String) {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let email = format!(
        "bd-{}-{}@board-digest.invalid",
        first.to_lowercase(),
        &tag[..12]
    );
    let sub = format!("board-digest-live:{tag}");
    let name = format!("{first}{} Bd", &tag[..6]);
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

/// Detached writes: poll until the condition holds (or 15 s pass).
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

async fn settle() {
    tokio::time::sleep(Duration::from_millis(1500)).await;
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

async fn move_to(state: &AppState, task: &str, cookie: &str, status: &str) {
    ok(
        state,
        "PUT",
        &format!("/api/tasks/{task}"),
        cookie,
        json!({ "status": status }),
    )
    .await;
}

async fn comment(state: &AppState, task: &str, cookie: &str, text: &str) {
    ok(
        state,
        "POST",
        &format!("/api/tasks/{task}/comments"),
        cookie,
        json!({ "content": text }),
    )
    .await;
}

async fn set(state: &AppState, board: &str, cookie: &str, body: Value) {
    ok(
        state,
        "PUT",
        &format!("/api/boards/{board}/notify"),
        cookie,
        body,
    )
    .await;
}

/// How many activity rows of `kind` the ticket holds — the comment door
/// writes its line from a detached task.
async fn activity(state: &AppState, task: &str, kind: &str) -> i64 {
    sqlx::query_scalar("select count(*) from task_activity where task_id = $1::uuid and type = $2")
        .bind(task)
        .bind(kind)
        .fetch_one(&state.pg)
        .await
        .unwrap()
}

/// Turn on Board activity's Email: the switch for the "Your boards" mail.
async fn board_mail_on(state: &AppState, user: &str) {
    sqlx::query(
        "update users set notify_prefs = notify_prefs || \
           '{\"channels\": {\"board_activity\": {\"email\": true}}}'::jsonb \
         where id = $1::uuid",
    )
    .bind(user)
    .execute(&state.pg)
    .await
    .unwrap();
}

/// The IANA zone whose wall clock reads `hour` at `now_ms`: a whole-hour
/// `Etc/GMT±N` (whose sign is POSIX's, inverted).
fn zone_reading(hour: i64, now_ms: i64) -> String {
    let utc_hour = (now_ms / 3_600_000).rem_euclid(24);
    let mut offset = (hour - utc_hour).rem_euclid(24);
    if offset > 12 {
        offset -= 24;
    }
    match offset {
        0 => "Etc/GMT".to_string(),
        o if o > 0 => format!("Etc/GMT-{o}"),
        o => format!("Etc/GMT+{}", -o),
    }
}

/// Put these people's clocks at the opening of the daily digest's window.
async fn in_the_slot(state: &AppState, now_ms: i64, users: &[&str]) {
    let config = talaria_digest::digest_config(&state.pg).await;
    let zone = zone_reading(config.hour, now_ms);
    for u in users {
        sqlx::query("update users set timezone = $2 where id = $1::uuid")
            .bind(u)
            .bind(&zone)
            .execute(&state.pg)
            .await
            .unwrap();
    }
}

/// A mail door that records every mail and answers from a script: the
/// answers in order, then ok for the rest.
fn door(script: Vec<GatedSendResult>) -> (SendBoardMail, Arc<Mutex<Vec<BoardMail>>>) {
    let sent: Arc<Mutex<Vec<BoardMail>>> = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(Mutex::new(script));
    let record = sent.clone();
    let send: SendBoardMail = Arc::new(move |m: BoardMail| {
        record.lock().unwrap().push(m);
        let answer = {
            let mut s = script.lock().unwrap();
            if s.is_empty() {
                GatedSendResult {
                    ok: true,
                    blocked: false,
                    error: None,
                }
            } else {
                s.remove(0)
            }
        };
        Box::pin(async move { answer })
    });
    (send, sent)
}

fn deps(state: &AppState, send: SendBoardMail) -> BoardDigestDeps {
    BoardDigestDeps {
        notify: NotifyDeps {
            pg: state.pg.clone(),
            realtime: talaria_api::realtime::RealtimeDeps::publish_only(None),
        },
        send,
    }
}

async fn pass(deps: &BoardDigestDeps, now_ms: i64, people: &[&str]) -> BoardDigestRun {
    let only: Vec<String> = people.iter().map(|s| s.to_string()).collect();
    run_board_digest_for(deps, now_ms, Some(&only))
        .await
        .expect("the pass runs")
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    id: String,
    kind: String,
    title: String,
    body: String,
    count: i32,
    group_key: Option<String>,
    alert: Option<String>,
    entity_type: Option<String>,
}

/// Every row one person holds about this board or one of its tickets.
async fn on_board(state: &AppState, user: &str, board: &str) -> Vec<Row> {
    sqlx::query_as(
        "select id::text as id, kind, title, body, count, group_key, alert, entity_type \
         from notifications \
         where user_id = $1::uuid and (href like $2 or href = $3) \
         order by created_at, seq",
    )
    .bind(user)
    .bind(format!("/boards/{board}/%"))
    .bind(format!("/boards/{board}"))
    .fetch_all(&state.pg)
    .await
    .unwrap()
}

fn digests(rows: &[Row]) -> Vec<Row> {
    rows.iter()
        .filter(|r| r.kind == "board-digest")
        .cloned()
        .collect()
}

// THE DAY'S DIGEST. Bob hears the whole board as a digest, with Board
// activity's Email on; Cy hears the tickets she is in as a digest, mail off;
// Dee has no setting. As things happen, Bob gets nothing and Cy only what
// points at her (her assignment). At the slot: one quiet row each — Bob's
// counts the board, minus his own new ticket and a sync's writes; Cy's only
// her ticket's move — and one mail, Bob's. A second tick files and mails
// nothing. Tomorrow's digest counts only what came after, in a row of its
// own: today's, unread, keeps its counts.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_digest_board_files_one_row_a_day_and_one_mail() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let (dee, dee_e, _) = mint(&state, "Dee", "member").await;
    let name = format!("bd day {}", &bob[..8]);
    let b = board(
        &state,
        &ada_c,
        &name,
        &[(&bob_e, "editor"), (&cy_e, "editor"), (&dee_e, "viewer")],
    )
    .await;
    set(
        &state,
        &b,
        &bob_c,
        json!({ "preset": "all", "cadence": "digest" }),
    )
    .await;
    set(
        &state,
        &b,
        &cy_c,
        json!({ "preset": "participating", "cadence": "digest" }),
    )
    .await;
    board_mail_on(&state, &bob).await;

    let t1 = ticket(
        &state,
        &b,
        &ada_c,
        json!({ "title": "Cy's", "assignees": [format!("user:{cy}")] }),
    )
    .await;
    let t2 = ticket(&state, &b, &ada_c, json!({ "title": "Nobody's" })).await;
    // Bob's own ticket is not news to Bob.
    ticket(&state, &b, &bob_c, json!({ "title": "Bob's own" })).await;
    move_to(&state, &t1, &ada_c, "in_progress").await;
    move_to(&state, &t2, &ada_c, "done").await;
    comment(&state, &t2, &ada_c, "closing this out").await;
    assert!(eventually!(activity(&state, &t2, "comment").await == 1));
    // A tracker sync's writes mirror another tracker: never counted.
    for (kind, line) in [
        ("status", "moved to in_progress"),
        ("comment", "imported 2 comments from ClickUp"),
    ] {
        sqlx::query(
            "insert into task_activity (task_id, actor, type, description) values ($1::uuid, $2, $3, $4)",
        )
        .bind(&t1)
        .bind(talaria_tasks_types::CLICKUP_SYNC_ACTOR)
        .bind(kind)
        .bind(line)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    settle().await;
    // As it happened: nothing for Bob, only the assignment for Cy.
    assert!(
        on_board(&state, &bob, &b).await.is_empty(),
        "a digest board files nothing as it happens: {:?}",
        on_board(&state, &bob, &b).await
    );
    let cy_now: Vec<String> = on_board(&state, &cy, &b)
        .await
        .into_iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(
        cy_now,
        ["task-assigned"],
        "what points at her is never held"
    );

    let now = talaria_api::agent_auth::now_ms();
    in_the_slot(&state, now, &[&bob, &cy, &dee]).await;
    let (send, sent) = door(Vec::new());
    let d = deps(&state, send);
    let run = pass(&d, now, &[&bob, &cy, &dee]).await;
    assert_eq!(run.considered, 2, "Dee chose no digest: {run:?}");
    assert_eq!(
        (run.people_filed, run.rows, run.mailed),
        (2, 2, 1),
        "{run:?}"
    );

    let bob_rows = on_board(&state, &bob, &b).await;
    assert_eq!(bob_rows.len(), 1, "{bob_rows:?}");
    let row = &bob_rows[0];
    assert_eq!(row.kind, "board-digest");
    assert_eq!(row.title, format!("Daily digest: {name}"));
    assert_eq!(
        row.body,
        "2 new \u{b7} 1 moved \u{b7} 1 completed \u{b7} 1 comment"
    );
    assert_eq!(row.alert.as_deref(), Some("feed"), "a digest never rings");
    assert_eq!(row.group_key, Some(format!("board:{b}:digest")));
    assert_eq!(row.entity_type.as_deref(), Some("board"));
    assert_eq!(row.count, 1);

    let cy_digest = digests(&on_board(&state, &cy, &b).await);
    assert_eq!(cy_digest.len(), 1, "{cy_digest:?}");
    assert_eq!(cy_digest[0].body, "1 moved", "only the ticket she is in");

    {
        let mails = sent.lock().unwrap();
        assert_eq!(mails.len(), 1, "Bob's mail only — Cy's Email is off");
        assert_eq!(mails[0].to, bob_e);
        assert_eq!(
            mails[0].subject,
            format!(
                "Talaria: {name} \u{2014} 2 new \u{b7} 1 moved \u{b7} 1 completed \u{b7} 1 comment"
            )
        );
        assert!(mails[0].text.contains(&name));
    }

    // A second tick inside the window: nothing filed, nothing mailed.
    let again = pass(&d, now + 15 * 60_000, &[&bob, &cy]).await;
    assert_eq!(again.already_done, 2, "{again:?}");
    assert_eq!((again.rows, again.mailed), (0, 0));
    assert_eq!(digests(&on_board(&state, &bob, &b).await)[0].count, 1);
    assert_eq!(sent.lock().unwrap().len(), 1);

    // Tomorrow: only what came after today's digest, in its own row — the
    // unread one from today keeps what it said.
    ticket(&state, &b, &ada_c, json!({ "title": "After the digest" })).await;
    let tomorrow = now + 24 * 3_600_000;
    let next = pass(&d, tomorrow, &[&bob, &cy]).await;
    assert_eq!(
        (next.rows, next.mailed),
        (1, 1),
        "Cy's tickets did not move: {next:?}"
    );
    let bob_digest = digests(&on_board(&state, &bob, &b).await);
    assert_eq!(bob_digest.len(), 2, "{bob_digest:?}");
    assert_eq!(bob_digest[0].id, row.id, "today's row, untouched");
    assert_eq!(bob_digest[0].body, row.body);
    assert_eq!(bob_digest[0].count, 1);
    assert_ne!(bob_digest[1].group_key, row.group_key);
    assert_eq!(bob_digest[1].body, "1 new");
    assert_eq!(bob_digest[1].count, 1);
    assert_eq!(
        sent.lock().unwrap()[1].subject,
        format!("Talaria: {name} \u{2014} 1 new")
    );
}

// THE MAIL IS OWED UNTIL IT GOES. The instance switch holding it, then a
// failed send, then a good one: the row is filed once, at the first tick, and
// each later tick inside the window offers the same mail again until it
// goes; after that, nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_held_or_failed_mail_is_sent_later_without_filing_again() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "bd retry", &[(&bob_e, "editor")]).await;
    set(
        &state,
        &b,
        &bob_c,
        json!({ "preset": "all", "cadence": "digest" }),
    )
    .await;
    board_mail_on(&state, &bob).await;
    ticket(&state, &b, &ada_c, json!({ "title": "One" })).await;
    settle().await;

    let now = talaria_api::agent_auth::now_ms();
    in_the_slot(&state, now, &[&bob]).await;
    let (send, sent) = door(vec![
        GatedSendResult {
            ok: false,
            blocked: true,
            error: None,
        },
        GatedSendResult {
            ok: false,
            blocked: false,
            error: Some("provider down".into()),
        },
    ]);
    let d = deps(&state, send);
    let first = pass(&d, now, &[&bob]).await;
    assert_eq!(
        (first.rows, first.mail_held, first.mailed),
        (1, 1, 0),
        "{first:?}"
    );
    let second = pass(&d, now + 15 * 60_000, &[&bob]).await;
    assert_eq!((second.rows, second.mail_failed), (0, 1), "{second:?}");
    let third = pass(&d, now + 30 * 60_000, &[&bob]).await;
    assert_eq!((third.rows, third.mailed), (0, 1), "{third:?}");
    let fourth = pass(&d, now + 45 * 60_000, &[&bob]).await;
    assert_eq!((fourth.already_done, fourth.mailed), (1, 0), "{fourth:?}");
    let rows = digests(&on_board(&state, &bob, &b).await);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 1, "filed once");
    let mails = sent.lock().unwrap();
    assert_eq!(mails.len(), 3, "offered three times, delivered once");
    assert!(mails.iter().all(|m| m.subject == mails[0].subject));
}

// ONLY WHAT THE SETTING COVERS. Bob's default for every board is a digest of
// all activity, and one board's own row says instant: that board files as it
// happens and is not on the digest. A ticket Bob watches himself reached him
// as it moved, so its move is not counted again. Eve chose a digest and then
// lost the board: dormant, nothing. A board where nothing happened that the
// setting covers sends nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn only_the_boards_and_tickets_the_setting_covers() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (eve, eve_e, eve_c) = mint(&state, "Eve", "member").await;
    let digest_b = board(
        &state,
        &ada_c,
        "bd covered",
        &[(&bob_e, "editor"), (&eve_e, "editor")],
    )
    .await;
    let instant_b = board(&state, &ada_c, "bd instant", &[(&bob_e, "editor")]).await;
    let quiet_b = board(&state, &ada_c, "bd quiet", &[(&bob_e, "editor")]).await;
    ok(
        &state,
        "PATCH",
        "/api/notifications",
        &bob_c,
        json!({ "boards": { "preset": "all", "cadence": "digest" } }),
    )
    .await;
    set(&state, &instant_b, &bob_c, json!({ "preset": "all" })).await;
    set(
        &state,
        &digest_b,
        &eve_c,
        json!({ "preset": "all", "cadence": "digest" }),
    )
    .await;
    ok(
        &state,
        "DELETE",
        &format!("/api/boards/{digest_b}/members"),
        &ada_c,
        json!({ "userId": eve }),
    )
    .await;

    let watched = ticket(&state, &digest_b, &ada_c, json!({ "title": "Watched" })).await;
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{watched}/watchers"),
        &bob_c,
        json!({ "watcher": bob_e }),
    )
    .await;
    let other = ticket(&state, &digest_b, &ada_c, json!({ "title": "Other" })).await;
    move_to(&state, &watched, &ada_c, "in_progress").await;
    move_to(&state, &other, &ada_c, "in_progress").await;
    ticket(&state, &instant_b, &ada_c, json!({ "title": "Instant" })).await;
    assert!(eventually!(
        on_board(&state, &bob, &digest_b)
            .await
            .iter()
            .any(|r| r.kind == "task-status")
    ));
    assert!(eventually!(
        on_board(&state, &bob, &instant_b)
            .await
            .iter()
            .any(|r| r.kind == "board-activity")
    ));
    settle().await;

    let now = talaria_api::agent_auth::now_ms();
    in_the_slot(&state, now, &[&bob, &eve]).await;
    let (send, sent) = door(Vec::new());
    let d = deps(&state, send);
    let run = pass(&d, now, &[&bob, &eve]).await;
    assert_eq!(run.considered, 2, "{run:?}");
    assert_eq!(
        (run.people_filed, run.rows),
        (1, 1),
        "Bob, one board: {run:?}"
    );
    assert_eq!(
        run.nothing_to_report, 1,
        "Eve: no board she can see: {run:?}"
    );
    assert_eq!(run.mailed, 0, "Bob's Email for Board activity is off");
    assert!(sent.lock().unwrap().is_empty());

    let covered = digests(&on_board(&state, &bob, &digest_b).await);
    assert_eq!(covered.len(), 1, "{covered:?}");
    assert_eq!(
        covered[0].body, "2 new \u{b7} 1 moved",
        "the watched ticket's move reached him as it happened"
    );
    assert!(digests(&on_board(&state, &bob, &instant_b).await).is_empty());
    assert!(digests(&on_board(&state, &bob, &quiet_b).await).is_empty());
    assert!(on_board(&state, &eve, &digest_b).await.is_empty());
}

// TWO CLAIMS AT ONCE, ONE WINNER — raced on one read mark, which two whole
// passes cannot be made to do on cue. The day: of two passes that read the
// same (absent) mark, one claims it. Its mail cannot be claimed while the day
// is filing; a claim that is not this pass's cannot mark the rows landed;
// then of two mail claims, one wins. A stale claim: of two passes that read
// it, one takes it over, and the pass it was taken from can no longer land
// its rows.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn two_claims_at_once_have_one_winner() {
    let state = live_app_state().await;
    let pg = &state.pg;
    let (bob, _, _) = mint(&state, "Bob", "member").await;
    let now = talaria_api::agent_auth::now_ms();
    let window = (now - DAY_MS, now);
    let day = "2026-11-20";

    let (one, two) = tokio::join!(
        seam::claim_day(pg, &bob, day, window, true, now),
        seam::claim_day(pg, &bob, day, window, true, now),
    );
    let won = [one.unwrap(), two.unwrap()];
    assert_eq!(won.iter().filter(|w| **w).count(), 1, "the day: {won:?}");
    assert!(
        !seam::claim_mail(pg, &bob, day).await.unwrap(),
        "no mail while the day is filing"
    );
    assert!(!seam::rows_landed(pg, &bob, day, now - 1).await.unwrap());
    assert!(seam::rows_landed(pg, &bob, day, now).await.unwrap());
    let (one, two) = tokio::join!(
        seam::claim_mail(pg, &bob, day),
        seam::claim_mail(pg, &bob, day),
    );
    let won = [one.unwrap(), two.unwrap()];
    assert_eq!(won.iter().filter(|w| **w).count(), 1, "the mail: {won:?}");

    // The next day's claim, left by a pass that died; two passes past the
    // stale bound read it and race to take it over.
    let next = "2026-11-21";
    assert!(
        seam::claim_day(pg, &bob, next, (now, now + DAY_MS), true, now + DAY_MS)
            .await
            .unwrap()
    );
    let later = now + DAY_MS + 15 * 60_000;
    let (one, two) = tokio::join!(
        seam::take_over(pg, &bob, next, now + DAY_MS, later),
        seam::take_over(pg, &bob, next, now + DAY_MS, later + 1),
    );
    let won = [one.unwrap(), two.unwrap()];
    assert_eq!(
        won.iter().filter(|w| **w).count(),
        1,
        "the takeover: {won:?}"
    );
    assert!(
        !seam::rows_landed(pg, &bob, next, now + DAY_MS)
            .await
            .unwrap(),
        "the pass it was taken from mails nothing"
    );
}

// A PASS THAT DIED MID-FILING. It claimed the day and filed one board's row
// of two, then died. A pass inside the stale bound leaves the claim (its
// owner may still be filing) and mails nothing about rows that are not
// there; the first tick past it takes the claim over, files only the missing
// row, and mails once; the tick after that has nothing to do.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_claim_a_dead_pass_left_is_taken_over_and_mailed_once() {
    let state = live_app_state().await;
    let pg = &state.pg;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let first = board(&state, &ada_c, "bd dead one", &[(&bob_e, "editor")]).await;
    let second = board(&state, &ada_c, "bd dead two", &[(&bob_e, "editor")]).await;
    for b in [&first, &second] {
        set(
            &state,
            b,
            &bob_c,
            json!({ "preset": "all", "cadence": "digest" }),
        )
        .await;
        ticket(&state, b, &ada_c, json!({ "title": "Here" })).await;
    }
    board_mail_on(&state, &bob).await;
    settle().await;

    let now = talaria_api::agent_auth::now_ms();
    in_the_slot(&state, now, &[&bob]).await;
    let day = seam::local_day(pg, &bob, now).await.unwrap();
    let (send, sent) = door(Vec::new());
    let d = deps(&state, send);
    // The dead pass: its claim, and the first board's row.
    assert!(
        seam::claim_day(pg, &bob, &day, (now - DAY_MS, now), true, now)
            .await
            .unwrap()
    );
    let landed = BoardDigest {
        board_id: first.clone(),
        name: "bd dead one".into(),
        counts: Counts {
            created: 1,
            ..Counts::default()
        },
    };
    assert_eq!(
        seam::file_rows(&d.notify, &bob, &day, &[landed])
            .await
            .unwrap(),
        1
    );

    let early = pass(&d, now + 5 * 60_000, &[&bob]).await;
    assert_eq!(
        (early.busy, early.rows, early.mailed),
        (1, 0, 0),
        "a fresh claim is left alone: {early:?}"
    );
    assert!(sent.lock().unwrap().is_empty(), "no mail while filing");

    let late = pass(&d, now + 15 * 60_000, &[&bob]).await;
    assert_eq!(
        (late.people_filed, late.rows, late.mailed),
        (1, 1, 1),
        "taken over: only the missing row, then the mail: {late:?}"
    );
    let again = pass(&d, now + 30 * 60_000, &[&bob]).await;
    assert_eq!(
        (again.already_done, again.rows, again.mailed),
        (1, 0, 0),
        "{again:?}"
    );
    for b in [&first, &second] {
        let rows = digests(&on_board(&state, &bob, b).await);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].count, 1, "filed once");
        assert_eq!(rows[0].body, "1 new");
    }
    let mails = sent.lock().unwrap();
    assert_eq!(mails.len(), 1, "mailed once");
    assert!(
        mails[0].subject.contains("on 2 boards"),
        "{}",
        mails[0].subject
    );
}

// DUE DATES ARE THRESHOLDS CROSSED, NOT A BACKLOG. Over a window in which
// nothing else happened: a ticket overdue since long before the window is
// not counted; one whose due date passed inside it is overdue (Bob's own,
// too); one that came within a day of its due date inside it is due soon;
// one due later, and a closed one, are not counted. A board whose only
// content is a stale overdue backlog has nothing to report.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn due_counts_are_thresholds_crossed_in_the_window() {
    let state = live_app_state().await;
    let pg = &state.pg;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, _) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "bd due", &[(&bob_e, "editor")]).await;
    let backlog = board(&state, &ada_c, "bd backlog", &[(&bob_e, "editor")]).await;
    let stale = ticket(&state, &b, &ada_c, json!({ "title": "Stale" })).await;
    let passed = ticket(
        &state,
        &b,
        &ada_c,
        json!({ "title": "Passed", "assignees": [format!("user:{bob}")] }),
    )
    .await;
    let soon = ticket(&state, &b, &ada_c, json!({ "title": "Soon" })).await;
    let far = ticket(&state, &b, &ada_c, json!({ "title": "Far" })).await;
    let closed = ticket(&state, &b, &ada_c, json!({ "title": "Closed" })).await;
    let old = ticket(&state, &backlog, &ada_c, json!({ "title": "Old" })).await;
    settle().await;

    // The window opens an hour after everything above was written.
    let t0 = talaria_api::agent_auth::now_ms();
    let (from, to) = (t0 + HOUR_MS, t0 + 25 * HOUR_MS);
    for (task, due_at) in [
        (&stale, t0 - 3 * DAY_MS),
        (&passed, t0 + 2 * HOUR_MS),
        (&soon, t0 + 30 * HOUR_MS),
        (&far, t0 + 60 * HOUR_MS),
        (&closed, t0 + 3 * HOUR_MS),
        (&old, t0 - 30 * DAY_MS),
    ] {
        sqlx::query(
            "update tasks set due_date = to_timestamp($2::bigint / 1000.0) where id = $1::uuid",
        )
        .bind(task)
        .bind(due_at)
        .execute(pg)
        .await
        .unwrap();
    }
    sqlx::query("update tasks set status = 'done' where id = $1::uuid")
        .bind(&closed)
        .execute(pg)
        .await
        .unwrap();
    let person = Recipient {
        id: bob.clone(),
        email: bob_e.clone(),
        prefs: None,
        timezone: None,
    };
    let raw = seam::raw_counts(pg, &b, &person, from, to).await.unwrap();
    assert_eq!(
        raw,
        Raw {
            due_soon: Tally { board: 1, mine: 0 },
            overdue: Tally { board: 1, mine: 1 },
            ..Raw::default()
        }
    );
    assert_eq!(
        seam::raw_counts(pg, &backlog, &person, from, to)
            .await
            .unwrap(),
        Raw::default(),
        "a stale backlog is not news"
    );
}

// A TICKET MOVED HERE counts from its arrival. On the board it came from it
// was moved and commented on; then it moved here into another column. Bob,
// who hears this board in full, gets "1 new" — not the earlier move or
// comment, and not the column line the board move wrote beside it. Cy, its
// assignee, who hears the tickets she is in, gets the arrival as one move.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_ticket_moved_here_counts_from_its_arrival() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let (cy, cy_e, cy_c) = mint(&state, "Cy", "member").await;
    let members = [(bob_e.as_str(), "editor"), (cy_e.as_str(), "editor")];
    let source = board(&state, &ada_c, "bd source", &members).await;
    let here = board(&state, &ada_c, "bd here", &members).await;
    set(
        &state,
        &here,
        &bob_c,
        json!({ "preset": "all", "cadence": "digest" }),
    )
    .await;
    set(
        &state,
        &here,
        &cy_c,
        json!({ "preset": "participating", "cadence": "digest" }),
    )
    .await;
    let t = ticket(
        &state,
        &source,
        &ada_c,
        json!({ "title": "Travels", "assignees": [format!("user:{cy}")] }),
    )
    .await;
    move_to(&state, &t, &ada_c, "in_progress").await;
    comment(&state, &t, &ada_c, "before it moved").await;
    assert!(eventually!(activity(&state, &t, "comment").await == 1));
    ok(
        &state,
        "POST",
        &format!("/api/tasks/{t}/move"),
        &ada_c,
        json!({ "boardId": here, "status": "inbox" }),
    )
    .await;
    assert_eq!(activity(&state, &t, "moved").await, 1);
    let column_lines: i64 = sqlx::query_scalar(
        "select count(*) from task_activity where task_id = $1::uuid and type = 'status' \
         and position('(moved boards' in description) > 0",
    )
    .bind(&t)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(column_lines, 1, "the board move's column line, beside it");
    settle().await;

    let now = talaria_api::agent_auth::now_ms();
    in_the_slot(&state, now, &[&bob, &cy]).await;
    let (send, _) = door(Vec::new());
    let d = deps(&state, send);
    let run = pass(&d, now, &[&bob, &cy]).await;
    assert_eq!(run.rows, 2, "{run:?}");
    let bob_rows = digests(&on_board(&state, &bob, &here).await);
    assert_eq!(bob_rows.len(), 1, "{bob_rows:?}");
    assert_eq!(bob_rows[0].body, "1 new");
    let cy_rows = digests(&on_board(&state, &cy, &here).await);
    assert_eq!(cy_rows.len(), 1, "{cy_rows:?}");
    assert_eq!(cy_rows[0].body, "1 moved");
}

// NO ADDRESS, STILL THE ROWS. A person whose account has no email address
// set a board to a digest with Board activity's Email on: the in-app row
// lands; no mail is attempted.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_person_without_an_address_gets_the_rows_and_no_mail() {
    let state = live_app_state().await;
    let (_ada, _, ada_c) = mint(&state, "Ada", "admin").await;
    let (bob, bob_e, bob_c) = mint(&state, "Bob", "member").await;
    let b = board(&state, &ada_c, "bd no address", &[(&bob_e, "editor")]).await;
    set(
        &state,
        &b,
        &bob_c,
        json!({ "preset": "all", "cadence": "digest" }),
    )
    .await;
    board_mail_on(&state, &bob).await;
    ticket(&state, &b, &ada_c, json!({ "title": "News" })).await;
    settle().await;
    sqlx::query("update users set email = null where id = $1::uuid")
        .bind(&bob)
        .execute(&state.pg)
        .await
        .unwrap();

    let now = talaria_api::agent_auth::now_ms();
    in_the_slot(&state, now, &[&bob]).await;
    let (send, sent) = door(Vec::new());
    let d = deps(&state, send);
    let run = pass(&d, now, &[&bob]).await;
    assert_eq!((run.rows, run.mailed), (1, 0), "{run:?}");
    assert!(sent.lock().unwrap().is_empty());
    let rows = digests(&on_board(&state, &bob, &b).await);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].body, "1 new");
}

// Live-DB proof for the cross-day verdict carry (cargo test -- --ignored).
// The report this file pins, 2026-09-10: an item the owner crossed off came
// back every morning, un-crossed, for as long as its source stood still — a
// check-off that only lasts the day is a nag with a date on it. The fix is
// that the open and the sweep's add branch skip a key whose newest entry on a
// PRIOR day is the owner's own verdict with an unchanged fingerprint, and
// these assertions drive the real open, the real mark and the real sweep
// against the real tables.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test brief_verdict_carry_live -- --ignored

use talaria_api::config::Config;
use talaria_api::daily_brief::{
    BriefUser, mark_brief_item, open_brief, real_brief_deps, sweep_brief,
};
use talaria_api::state::AppState;

async fn pg() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    sqlx::PgPool::connect(&url).await.expect("connect")
}

/// Redis on a dead port, exactly like brief_item_live: the appends publish
/// events that degrade to no-ops, and this file is about the rows.
async fn app_state() -> AppState {
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").unwrap_or_default(),
        "redis://127.0.0.1:1".into(),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

/// A user with the one thing the brief pipeline demands before it writes
/// anything: a personal assistant. The model string is unique per fixture
/// (agent_defs.model is UNIQUE) and names no real model, so the lede's
/// harness call fails and the fallback lede is written — this file is about
/// items, not prose.
async fn fabricate_user(pg: &sqlx::PgPool, tag: &str) -> String {
    let sub = format!("brief-carry:{tag}:{}", uuid::Uuid::new_v4());
    sqlx::query("insert into users (sub) values ($1)")
        .bind(&sub)
        .execute(pg)
        .await
        .unwrap();
    let id: String = sqlx::query_scalar("select id::text from users where sub = $1")
        .bind(&sub)
        .fetch_one(pg)
        .await
        .unwrap();
    sqlx::query(
        "insert into agent_defs (slug, department, model, display_name, owner_user_id) \
         values ($1, 'personal', $2, 'Carry Test Assistant', $3::uuid)",
    )
    .bind(format!("brief-carry-{tag}-{}", uuid::Uuid::new_v4()))
    .bind(format!("brief-carry-{}-{}", tag, uuid::Uuid::new_v4()))
    .bind(&id)
    .execute(pg)
    .await
    .unwrap();
    id
}

/// A blocked task on the user's own board — exactly the shape task_items
/// reports, so the real snapshot picks it up without any fixture reaching
/// into the brief tables to fake a live source.
async fn fabricate_blocked_task(pg: &sqlx::PgPool, owner: &str, title: &str) -> String {
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('brief carry live', $1::uuid) returning id::text",
    )
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board)
    .bind(owner)
    .execute(pg)
    .await
    .unwrap();
    let (task,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, title, status, priority, created_by) \
         values ($1::uuid, $2, 'blocked', 'medium', 'live-test') returning id::text",
    )
    .bind(&board)
    .bind(title)
    .fetch_one(pg)
    .await
    .unwrap();
    task
}

/// The prior day's checked-off row, appended exactly as mark_brief_item
/// writes it: kind 'checked', the SOURCE's fingerprint carried forward, and
/// a supersedes pointer at the live item entry the real open produced.
async fn append_checked_verdict(pg: &sqlx::PgPool, brief: &str, key: &str, fingerprint: &str) {
    let item: Option<(String,)> = sqlx::query_as(
        "select id::text from daily_brief_entries \
         where brief_id = $1::uuid and source_key = $2 and kind = 'item' limit 1",
    )
    .bind(brief)
    .bind(key)
    .fetch_optional(pg)
    .await
    .unwrap();
    let Some((item_id,)) = item else {
        panic!("the prior brief never listed {key}; the fixture is broken");
    };
    sqlx::query(
        "insert into daily_brief_entries \
           (brief_id, seq, kind, section, source_key, source_type, source_id, fingerprint, \
            supersedes, priority, status_label, title, body, evidence) \
         values ($1::uuid, (select coalesce(max(seq), 0) + 1 from daily_brief_entries where brief_id = $1::uuid), \
                 'checked', 'action', $2, 'task', null, $3, $4::uuid, 'ok', 'CHECKED OFF', 'x', '', '[]'::jsonb)",
    )
    .bind(brief)
    .bind(key)
    .bind(fingerprint)
    .bind(item_id)
    .execute(pg)
    .await
    .unwrap();
    // The seq counter lives on the brief row and is claimed by UPDATE, so a
    // raw INSERT has to hand it back what it took — else the next real
    // append claims the same seq and dies on the unique index.
    sqlx::query(
        "update daily_briefs set last_seq = greatest(last_seq, \
           (select coalesce(max(seq), 0) from daily_brief_entries where brief_id = $1::uuid)) \
         where id = $1::uuid",
    )
    .bind(brief)
    .execute(pg)
    .await
    .unwrap();
}

async fn entry_kinds(pg: &sqlx::PgPool, brief: &str, key: &str) -> Vec<String> {
    sqlx::query_scalar(
        "select kind from daily_brief_entries where brief_id = $1::uuid and source_key = $2 order by seq",
    )
    .bind(brief)
    .bind(key)
    .fetch_all(pg)
    .await
    .unwrap()
}

async fn fingerprint_of(pg: &sqlx::PgPool, brief: &str, key: &str) -> String {
    let fp: Option<String> = sqlx::query_scalar(
        "select fingerprint from daily_brief_entries \
         where brief_id = $1::uuid and source_key = $2 and kind = 'item' limit 1",
    )
    .bind(brief)
    .bind(key)
    .fetch_one(pg)
    .await
    .unwrap();
    fp.expect("the open stamps every item with a fingerprint")
}

async fn teardown(pg: &sqlx::PgPool, tag: &str) {
    sqlx::query(
        "delete from daily_brief_entries where brief_id in \
           (select id from daily_briefs where user_id in \
              (select id from users where sub like $1))",
    )
    .bind(format!("brief-carry:{tag}:%"))
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "delete from daily_briefs where user_id in \
           (select id from users where sub like $1)",
    )
    .bind(format!("brief-carry:{tag}:%"))
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "delete from agent_defs where owner_user_id in \
           (select id from users where sub like $1)",
    )
    .bind(format!("brief-carry:{tag}:%"))
    .execute(pg)
    .await
    .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("brief-carry:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

const DAY_MS: i64 = 24 * 3_600_000;

/// THE REPORT: a task checked off on an earlier day, its source unmoved, is
/// absent from today's document. Then the way back: a restore lands on the
/// prior day's page (today's fold has never heard of the key), and the next
/// sweep brings the line to today as new.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_crossed_off_item_stays_off_and_a_restore_brings_it_back() {
    let pg = pg().await;
    let owner = fabricate_user(&pg, "carry").await;
    let task = fabricate_blocked_task(&pg, &owner, "Still blocked, still dismissed").await;
    let key = format!("task:{task}");
    let user = BriefUser {
        id: owner.clone(),
        email: None,
        name: None,
        role: "member".into(),
        timezone: None,
    };
    let state = app_state().await;
    let deps = real_brief_deps(&state).await;

    // The prior day's document, opened through the real path two days back,
    // carries the task as a live item with its real fingerprint.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_millis() as i64;
    let prior = open_brief(&deps, &user, now - 2 * DAY_MS, None)
        .await
        .expect("the prior day opens")
        .brief_id
        .expect("a fresh brief id");
    assert_eq!(
        entry_kinds(&pg, &prior, &key).await,
        vec!["item".to_string()],
        "the fixture's premise: the prior day listed the task"
    );
    let fingerprint = fingerprint_of(&pg, &prior, &key).await;
    append_checked_verdict(&pg, &prior, &key, &fingerprint).await;

    // TODAY: the same live source, the same fingerprint — and no line.
    let today = open_brief(&deps, &user, now, None)
        .await
        .expect("today opens")
        .brief_id
        .expect("today's brief id");
    assert!(
        entry_kinds(&pg, &today, &key).await.is_empty(),
        "a checked-off item with an unchanged source does not come back"
    );

    // The way back. Restore resolves to the document the line is on — the
    // prior day's, because today never listed it — and appends there.
    let mark = mark_brief_item(&deps, &user, &key, "restore", None)
        .await
        .expect("restore resolves");
    assert!(mark.ok, "the prior page still answers for its own lines");
    assert_eq!(
        entry_kinds(&pg, &prior, &key).await,
        vec![
            "item".to_string(),
            "checked".to_string(),
            "change".to_string()
        ],
        "restore writes its change where the verdict was"
    );

    // And the next sweep, seeing a non-terminal latest entry, brings the
    // line to today as new.
    sweep_brief(&deps, &user, now)
        .await
        .expect("the sweep runs");
    assert_eq!(
        entry_kinds(&pg, &today, &key).await,
        vec!["item".to_string()],
        "a restored line reappears on the day it was restored"
    );

    teardown(&pg, "carry").await;
}

/// The other edge the report demands: a source that MOVED under the verdict
/// is new information, and the item comes back fingerprint or no.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_verdict_on_a_stale_fingerprint_does_not_suppress() {
    let pg = pg().await;
    let owner = fabricate_user(&pg, "drift").await;
    let task = fabricate_blocked_task(&pg, &owner, "Blocked differently now").await;
    let key = format!("task:{task}");
    let user = BriefUser {
        id: owner.clone(),
        email: None,
        name: None,
        role: "member".into(),
        timezone: None,
    };
    let state = app_state().await;
    let deps = real_brief_deps(&state).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_millis() as i64;
    let prior = open_brief(&deps, &user, now - 2 * DAY_MS, None)
        .await
        .expect("the prior day opens")
        .brief_id
        .expect("a fresh brief id");
    // A verdict stamped over yesterday's world; the source has since moved.
    append_checked_verdict(&pg, &prior, &key, "the-fingerprint-of-yesterday").await;

    let today = open_brief(&deps, &user, now, None)
        .await
        .expect("today opens")
        .brief_id
        .expect("today's brief id");
    assert_eq!(
        entry_kinds(&pg, &today, &key).await,
        vec!["item".to_string()],
        "a moved source reopens the question the verdict closed"
    );

    teardown(&pg, "drift").await;
}

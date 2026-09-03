// Live-DB proof for the brief item mark (cargo test -- --ignored). The bug
// this file pins, reported on the live fleet 2026-09-03: the read serves the
// most recent readable document when today's has not opened yet — get_brief's
// NEVER-'pending'-OVER-A-READABLE-DOCUMENT rule — and promises that anything
// the owner checks off there is real work. But the mark resolved ONLY today's
// row, so on that served page every check, dismiss and restore answered 404
// ("no brief today") into a document that was not the one on screen. The fix
// is the read's own resolution: today's row when it exists, else the most
// recent readable one — and that is what these assertions drive, against the
// real fold, the real append, the real 48-hour recency window.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test brief_item_live -- --ignored

use talaria_api::config::Config;
use talaria_api::daily_brief::{BriefUser, mark_brief_item, real_brief_deps};
use talaria_api::state::AppState;

async fn pg() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    sqlx::PgPool::connect(&url).await.expect("connect")
}

/// An AppState exactly the way the other live tests build one. Redis points
/// at a dead port on purpose: the mark's notify publish degrades to a no-op
/// without a connection, and this file is about the rows, not the socket.
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

async fn fabricate_user(pg: &sqlx::PgPool, tag: &str) -> String {
    let sub = format!("brief-live:{tag}:{}", uuid::Uuid::new_v4());
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
    id
}

/// A readable brief the read would serve as "the current one": a row with
/// entries, created now (inside the 48h recency window), dated well BEHIND
/// today — three days, so no timezone the org config could resolve to (any
/// IANA zone is within a day of UTC) can mistake it for today's document.
/// That is the pre-noon reality the fleet hit: yesterday's page on screen,
/// no row for the day that has started.
async fn fabricate_served_brief(
    pg: &sqlx::PgPool,
    user: &str,
    days_back: i32,
    key: &str,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "insert into daily_briefs (id, user_id, brief_date, last_seq) \
         values ($1::uuid, $2::uuid, current_date - $3::int, 5)",
    )
    .bind(&id)
    .bind(user)
    .bind(days_back)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into daily_brief_entries \
           (brief_id, seq, kind, section, source_key, title, body, evidence) \
         values ($1::uuid, 1, 'item', 'action', $2, 'A line worth closing', \
                 'The body of it.', '[]'::jsonb)",
    )
    .bind(&id)
    .bind(key)
    .execute(pg)
    .await
    .unwrap();
    id
}

async fn checked_rows(pg: &sqlx::PgPool, brief: &str, key: &str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(
        "select count(*) from daily_brief_entries \
         where brief_id = $1::uuid and source_key = $2 and kind = 'checked'",
    )
    .bind(brief)
    .bind(key)
    .fetch_one(pg)
    .await
    .unwrap();
    n
}

/// THE REPORT, as a wire contract: no brief has opened for today, the page on
/// screen is the most recent readable one, and the owner's verdict on a line
/// OF THAT PAGE must land — not 404 into a document that does not exist.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_mark_lands_on_the_document_the_read_serves() {
    let pg = pg().await;
    let owner = fabricate_user(&pg, "mark-owner").await;
    // Two readable documents, the newer one (two days back) is the one the
    // read serves; the older one (three days back) stands for a stale tab.
    let stale = fabricate_served_brief(&pg, &owner, 3, "brief-live:stale").await;
    let served = fabricate_served_brief(&pg, &owner, 2, "brief-live:served").await;

    let state = app_state().await;
    let deps = real_brief_deps(&state).await;
    let user = BriefUser {
        id: owner.clone(),
        email: None,
        name: None,
        role: "member".into(),
        timezone: None,
    };

    // The verdict on the SERVED page's line: ok. Pre-fix this was the 404 —
    // refused with "no brief today".
    let mark = mark_brief_item(&deps, &user, "brief-live:served", "check", None)
        .await
        .unwrap();
    assert!(mark.ok, "the served document takes the owner's verdict");
    assert_eq!(
        checked_rows(&pg, &served, "brief-live:served").await,
        1,
        "one checked row appended to the document that owns the line"
    );

    // The same click again: still ok, still ONE row — a double-click must not
    // put two identical strike-throughs in the timeline.
    let again = mark_brief_item(&deps, &user, "brief-live:served", "check", None)
        .await
        .unwrap();
    assert!(again.ok);
    assert_eq!(checked_rows(&pg, &served, "brief-live:served").await, 1);

    // A key from the OLDER document: refused. The fallback follows the read's
    // row choice — the newest readable page — so a stale tab still misses,
    // exactly as it would against today's row.
    let stale_mark = mark_brief_item(&deps, &user, "brief-live:stale", "check", None)
        .await
        .unwrap();
    assert!(!stale_mark.ok, "an older document is not the served one");
    assert_eq!(
        checked_rows(&pg, &stale, "brief-live:stale").await,
        0,
        "a refused mark appends nothing"
    );

    for brief in [&stale, &served] {
        sqlx::query("delete from daily_brief_entries where brief_id = $1::uuid")
            .bind(brief)
            .execute(&pg)
            .await
            .unwrap();
        sqlx::query("delete from daily_briefs where id = $1::uuid")
            .bind(brief)
            .execute(&pg)
            .await
            .unwrap();
    }
    sqlx::query("delete from users where sub like $1")
        .bind("brief-live:mark-owner:%")
        .execute(&pg)
        .await
        .unwrap();
}

/// The refusal that remains: a reader with NO readable document at all —
/// nothing today, nothing recent — still gets a clean "no", not an error.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reader_with_no_readable_document_gets_a_clean_refusal() {
    let pg = pg().await;
    let owner = fabricate_user(&pg, "mark-bare").await;
    let state = app_state().await;
    let deps = real_brief_deps(&state).await;
    let user = BriefUser {
        id: owner.clone(),
        email: None,
        name: None,
        role: "member".into(),
        timezone: None,
    };
    let mark = mark_brief_item(&deps, &user, "brief-live:anything", "check", None)
        .await
        .unwrap();
    assert!(!mark.ok, "no document anywhere means no verdict to record");

    sqlx::query("delete from users where sub like $1")
        .bind("brief-live:mark-bare:%")
        .execute(&pg)
        .await
        .unwrap();
}

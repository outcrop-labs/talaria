// Live-DB proof for the brief's write path (cargo test -- --ignored).
//
// The report this file pins, 2026-11-20: marking an inbox line handled took
// seconds per click on an instance whose disk waits on every flush. The click
// was two commits (the seq claim, then the entry), every page read could find
// a sweep due and run it inline, and nothing stopped a read and the job from
// sweeping the same brief at once. The fixes these assertions drive, against
// the real tables:
//
//   · an append is ONE transaction — the seq claim and the whole batch share
//     one xmin, the brief row's included;
//   · a sweep claims the brief before it starts, so two at once append one
//     batch, not two;
//   · POST /api/brief/item answers with the page, in exactly the shape
//     GET /api/brief serves.
//
// Run locally against a dev stack:
//
//   source ui/.env && cargo test -p talaria-api --test it brief_append_live:: -- --ignored

use serde_json::{Value, json};
use talaria_api::daily_brief::config::{brief_config, brief_window};
use talaria_api::daily_brief::types::NewEntry;
use talaria_api::daily_brief::{BriefUser, append_entries, real_brief_deps, sweep_if_due};

use crate::support::{app_state, call_json, live_app_state, minted_member_with_email, pg};

const PREFIX: &str = "brief-append";

/// A user whose stored zone is UTC, so the date the brief engine resolves for
/// them is the one this file computes.
async fn fabricate_user(pg: &sqlx::PgPool, tag: &str) -> String {
    let sub = format!("{PREFIX}:{tag}:{}", uuid::Uuid::new_v4());
    sqlx::query_scalar("insert into users (sub, timezone) values ($1, 'UTC') returning id::text")
        .bind(&sub)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// Today's date as the engine reads it for a UTC reader.
async fn today(pg: &sqlx::PgPool) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_millis() as i64;
    let config = brief_config(pg).await;
    brief_window(&config, "UTC", now).date
}

/// A brief for `user` on `date` with one open line per key, its artifact
/// already made — so the detached mirror an append fires only rewrites the
/// artifact and never touches the brief row these tests read. `swept_now`
/// stamps last_swept_at so no read finds a sweep due.
async fn fabricate_brief(
    pg: &sqlx::PgPool,
    user: &str,
    date: &str,
    keys: &[&str],
    swept_now: bool,
) -> String {
    let artifact: String = sqlx::query_scalar(
        "insert into artifacts (title, owner_user_id) values ('brief append live', $1::uuid) returning id::text",
    )
    .bind(user)
    .fetch_one(pg)
    .await
    .unwrap();
    let brief: String = sqlx::query_scalar(
        "insert into daily_briefs (user_id, brief_date, zone, artifact_id, last_seq, read_seq, last_swept_at) \
         values ($1::uuid, $2::date, 'UTC', $3::uuid, $4::int, $4::int, case when $5 then now() end) \
         returning id::text",
    )
    .bind(user)
    .bind(date)
    .bind(&artifact)
    .bind(keys.len() as i32)
    .bind(swept_now)
    .fetch_one(pg)
    .await
    .unwrap();
    for (i, key) in keys.iter().enumerate() {
        let source_id = key.split_once(':').map(|(_, id)| id).unwrap_or(key);
        sqlx::query(
            "insert into daily_brief_entries \
               (brief_id, seq, kind, section, source_key, source_type, source_id, fingerprint, \
                priority, status_label, title, body, evidence) \
             values ($1::uuid, $2::int, 'item', 'action', $3, 'task', $4, 'fp', 'p1', 'BLOCKED', \
                     'A line worth closing', 'The body of it.', '[]'::jsonb)",
        )
        .bind(&brief)
        .bind(i as i32 + 1)
        .bind(key)
        .bind(source_id)
        .execute(pg)
        .await
        .unwrap();
    }
    brief
}

async fn teardown(pg: &sqlx::PgPool, sub_like: &str) {
    let users = "(select id from users where sub like $1)";
    for sql in [
        format!(
            "delete from daily_brief_entries where brief_id in (select id from daily_briefs where user_id in {users})"
        ),
        format!("delete from daily_briefs where user_id in {users}"),
        format!("delete from artifacts where owner_user_id in {users}"),
        "delete from users where sub like $1".to_string(),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(sub_like)
            .execute(pg)
            .await
            .unwrap();
    }
}

fn brief_user(id: &str) -> BriefUser {
    BriefUser {
        id: id.to_string(),
        email: None,
        name: None,
        role: "member".into(),
        timezone: None,
    }
}

/// ONE COMMIT PER APPEND: three rows, the seq claim with them, and the brief
/// row all carry the same xmin — the id of the one transaction that wrote
/// them. Row-by-row autocommits gave every entry its own.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_append_of_three_rows_is_one_commit() {
    let pg = pg().await;
    teardown(&pg, &format!("{PREFIX}:xmin:%")).await;
    let owner = fabricate_user(&pg, "xmin").await;
    let date = today(&pg).await;
    let brief = fabricate_brief(&pg, &owner, &date, &["task:one"], true).await;
    let first: String = sqlx::query_scalar(
        "select id::text from daily_brief_entries where brief_id = $1::uuid and seq = 1",
    )
    .bind(&brief)
    .fetch_one(&pg)
    .await
    .unwrap();

    let state = app_state().await;
    let deps = real_brief_deps(&state).await;
    let badge = json!({ "label": "DUE TODAY", "tone": "danger" });
    let evidence = json!([{ "label": "Task", "text": "details" }]);
    let rows = vec![
        NewEntry::narrative("note", "action", "", "Three things moved."),
        NewEntry {
            kind: "change".into(),
            section: "action".into(),
            source_key: Some("task:one".into()),
            source_type: Some("task".into()),
            source_id: Some("one".into()),
            source_href: Some("/boards/b/one".into()),
            fingerprint: Some("fp-2".into()),
            supersedes: Some(first.clone()),
            priority: Some("p0".into()),
            status_label: Some("BLOCKED".into()),
            badge: Some(badge.clone()),
            title: "A line worth closing".into(),
            body: "Moved.".into(),
            evidence: evidence.clone(),
        },
        NewEntry {
            kind: "item".into(),
            section: "comms".into(),
            source_key: Some("channel:two".into()),
            source_type: Some("channel".into()),
            source_id: Some("two".into()),
            source_href: None,
            fingerprint: Some("fp-two".into()),
            supersedes: None,
            priority: Some("p2".into()),
            status_label: Some("WAITING".into()),
            badge: None,
            title: "Someone is waiting on you".into(),
            body: String::new(),
            evidence: json!([]),
        },
    ];
    let written = append_entries(&deps, &brief, &owner, rows)
        .await
        .expect("the append lands");

    // Returned oldest first, at the next three seqs, as one batch.
    assert_eq!(
        written.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert!(written[0].batch.is_some());
    assert!(written.iter().all(|e| e.batch == written[0].batch));
    assert_eq!(
        written.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        vec!["note", "change", "item"]
    );
    // The typed columns survive the trip: the uuid pointer, the jsonb pair.
    assert_eq!(written[1].supersedes.as_deref(), Some(first.as_str()));
    assert_eq!(written[1].badge.as_ref(), Some(&badge));
    assert_eq!(written[1].evidence, evidence);
    assert_eq!(written[2].badge, None);
    assert_eq!(written[2].source_href, None);

    let last_seq: i32 = sqlx::query_scalar("select last_seq from daily_briefs where id = $1::uuid")
        .bind(&brief)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(last_seq, 4, "the cursor moved by exactly the batch");

    let commits: i64 = sqlx::query_scalar(
        "select count(distinct x) from ( \
           select xmin::text as x from daily_brief_entries where brief_id = $1::uuid and seq > 1 \
           union all \
           select xmin::text from daily_briefs where id = $1::uuid) t",
    )
    .bind(&brief)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        commits, 1,
        "the seq claim and all three rows are one transaction"
    );

    teardown(&pg, &format!("{PREFIX}:xmin:%")).await;
}

/// ONE SWEEP AT A TIME: two reads that both find the brief due sweep it once.
/// The line's task does not exist, so a sweep resolves it — and without the
/// claim both sweeps did, appending two 'resolved' rows for one line in two
/// batches.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn two_sweeps_at_once_append_one_batch() {
    let pg = pg().await;
    teardown(&pg, &format!("{PREFIX}:claim:%")).await;
    let owner = fabricate_user(&pg, "claim").await;
    let date = today(&pg).await;
    let key = format!("task:{}", uuid::Uuid::new_v4());
    let brief = fabricate_brief(&pg, &owner, &date, &[key.as_str()], false).await;

    let state = app_state().await;
    let deps = real_brief_deps(&state).await;
    let user = brief_user(&owner);
    let (a, b) = tokio::join!(sweep_if_due(&deps, &user), sweep_if_due(&deps, &user));
    let ran: Vec<_> = [a.expect("sweep a"), b.expect("sweep b")]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(ran.len(), 1, "exactly one of the two sweeps ran");
    assert_eq!(ran[0].resolved, 1);

    let resolved: i64 = sqlx::query_scalar(
        "select count(*) from daily_brief_entries \
         where brief_id = $1::uuid and source_key = $2 and kind = 'resolved'",
    )
    .bind(&brief)
    .bind(&key)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(resolved, 1, "one resolution for one line");
    let batches: i64 = sqlx::query_scalar(
        "select count(distinct batch) from daily_brief_entries where brief_id = $1::uuid and seq > 1",
    )
    .bind(&brief)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(batches, 1, "one batch appended");

    // The claim stamped the brief: a third read right after finds nothing due.
    let again = sweep_if_due(&deps, &user).await.expect("sweep c");
    assert!(again.is_none(), "inside the window the claim stands");

    teardown(&pg, &format!("{PREFIX}:claim:%")).await;
}

/// THE ANSWER CARRIES THE PAGE: POST /api/brief/item's `brief` is exactly
/// what GET /api/brief serves right after — the same document, key for key —
/// so the client can put it straight into the cache the GET fills.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_mark_answers_with_the_page_get_serves() {
    let state = live_app_state().await;
    let email = format!("{}@brief-append.invalid", uuid::Uuid::new_v4());
    let (owner, cookie) =
        minted_member_with_email(&state, &format!("{PREFIX}-route"), &email).await;
    sqlx::query("update users set timezone = 'UTC' where id = $1::uuid")
        .bind(&owner)
        .execute(&state.pg)
        .await
        .unwrap();
    let date = today(&state.pg).await;
    let brief = fabricate_brief(&state.pg, &owner, &date, &["task:route"], true).await;

    let (status, posted) = call_json(
        &state,
        "POST",
        "/api/brief/item",
        Some(&cookie),
        Some(json!({ "sourceKey": "task:route", "action": "check", "tz": "UTC" })),
    )
    .await;
    assert_eq!(status, 200, "{posted}");
    assert_eq!(posted["ok"], true);
    let page = &posted["brief"];
    assert_eq!(page["id"], Value::String(brief.clone()));
    assert_eq!(page["lastSeq"], 2);
    let line = &page["sections"][0]["lines"][0];
    assert_eq!(line["key"], "task:route");
    assert_eq!(line["resolved"], true);
    assert_eq!(line["current"]["kind"], "checked");
    assert_eq!(line["current"]["statusLabel"], "CHECKED OFF");

    let (status, got) = call_json(&state, "GET", "/api/brief?tz=UTC", Some(&cookie), None).await;
    assert_eq!(status, 200, "{got}");
    assert_eq!(page, &got, "the POST's page is the GET's page");
    // Key order too: the client gets the same document either way.
    assert_eq!(page.to_string(), got.to_string());

    // A stale line still answers 404, with no page.
    let (status, refused) = call_json(
        &state,
        "POST",
        "/api/brief/item",
        Some(&cookie),
        Some(json!({ "sourceKey": "task:not-on-this-page", "action": "check", "tz": "UTC" })),
    )
    .await;
    assert_eq!(status, 404, "{refused}");
    assert!(refused.get("brief").is_none());

    teardown(&state.pg, &format!("{PREFIX}-route:%")).await;
}

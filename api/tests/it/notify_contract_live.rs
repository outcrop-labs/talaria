// Live-DB proof of the notification contract's write path (cargo test --
// --ignored): the per-user, commit-ordered seq under concurrent writers
// (I-10), the group-key upsert and its window, adoption of a pre-contract
// row, the entity clears, and the ack. Postgres decides every one of these —
// the row lock, the partial unique index, ON CONFLICT — so only a real
// database can confirm them. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it notify_contract_live:: -- --ignored --test-threads=1
//
// The write-path timing (C32) runs against the scratch database the
// measurement loads, and only when C32_DATABASE_URL names it:
//
//   C32_DATABASE_URL=postgres://…/c32 cargo test -p talaria-api --test it \
//     notify_contract_live::write_path_timing -- --ignored --nocapture

use crate::support::pg;
use futures_util::FutureExt;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use talaria_api::notify::{
    Coalesce, NotificationInput, NotifyDeps, NotifyEntity, NotifyMeta, add_notification,
    add_notification_with, clear_entity, defer, entity, group_key, mark_notifications_read,
    resolve_entity,
};
use talaria_api::realtime::RealtimeDeps;

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// Every (topic, payload) the writes publish, in order.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<(String, String)>>>);

impl Captured {
    fn deps(&self, pg: &PgPool) -> NotifyDeps {
        let seen = self.0.clone();
        NotifyDeps {
            pg: pg.clone(),
            realtime: RealtimeDeps {
                publish: Arc::new(move |topic: &str, payload: &str| {
                    seen.lock()
                        .unwrap()
                        .push((topic.to_string(), payload.to_string()));
                }),
                subscribe: Arc::new(|_| {
                    async { tokio::sync::mpsc::channel::<String>(1).1 }.boxed()
                }),
            },
        }
    }
    fn on(&self, topic: &str) -> Vec<serde_json::Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == topic)
            .map(|(_, p)| serde_json::from_str(p).unwrap())
            .collect()
    }
}

async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'notify-contract-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

/// A throwaway member whose every class routes in-app (no mail is queued).
async fn person(pg: &PgPool, tag: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role, notify_prefs) \
         values ($1, $2, $3, 'member', $4::jsonb) returning id::text",
    )
    .bind(format!("notify-contract-{tag}"))
    .bind(format!("notify-contract-{tag}@test.invalid"))
    .bind(format!("Contract {tag}"))
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

fn input<'a>(kind: &'a str, title: &'a str, body: &'a str, href: &'a str) -> NotificationInput<'a> {
    NotificationInput {
        kind,
        title,
        body: Some(body),
        href: Some(href),
    }
}

/// A pool wide enough for the concurrent writers plus the detached nudges.
async fn wide_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect(crate::support::DATABASE_URL_EXPECT);
    PgPoolOptions::new()
        .max_connections(24)
        .connect(&url)
        .await
        .unwrap()
}

// ── I-10: seq order IS commit order ─────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn seq_commit_order() {
    let pg = wide_pool().await;
    cleanup(&pg).await;
    let ada = person(&pg, "seq").await;
    let base = notify_seq(&pg, &ada).await;
    const WRITERS: usize = 8;
    const EACH: usize = 2_000;
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // The reader: one statement per sample, so the cursor and the count come
    // from ONE snapshot. A violation is a snapshot that sees notify_seq = N
    // but fewer (or more) than N - base rows at or below it.
    let reader = {
        let pg = pg.clone();
        let ada = ada.clone();
        let done = done.clone();
        tokio::spawn(async move {
            let (mut samples, mut during, mut violations) = (0u64, 0u64, Vec::new());
            loop {
                let writing = done.load(std::sync::atomic::Ordering::SeqCst) < WRITERS;
                let (seq, count): (i64, i64) = sqlx::query_as(
                    "select u.notify_seq, \
                            (select count(*) from notifications n \
                              where n.user_id = u.id and n.seq <= u.notify_seq and n.seq is not null) \
                     from users u where u.id = $1::uuid",
                )
                .bind(&ada)
                .fetch_one(&pg)
                .await
                .unwrap();
                samples += 1;
                if writing {
                    during += 1;
                }
                if count != seq - base {
                    violations.push((seq, count));
                }
                if !writing && samples >= 10_000 {
                    break;
                }
            }
            (samples, during, violations)
        })
    };

    let started = Instant::now();
    let mut writers = Vec::new();
    for w in 0..WRITERS {
        let deps = NotifyDeps::publishing(pg.clone(), None);
        let ada = ada.clone();
        let done = done.clone();
        writers.push(tokio::spawn(async move {
            for i in 0..EACH {
                let title = format!("writer {w} row {i}");
                add_notification(&deps, &ada, &input("mention", &title, "b", "/boards/x"))
                    .await
                    .unwrap();
            }
            done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
    }
    for w in writers {
        w.await.unwrap();
    }
    let wrote = started.elapsed();
    let (samples, during, violations) = reader.await.unwrap();

    let seqs: Vec<i64> = sqlx::query_scalar(
        "select seq from notifications where user_id = $1::uuid and seq is not null order by seq",
    )
    .bind(&ada)
    .fetch_all(&pg)
    .await
    .unwrap();
    let total = (WRITERS * EACH) as i64;
    let last = notify_seq(&pg, &ada).await;
    println!(
        "seq_commit_order: {WRITERS} writers x {EACH} rows in {:.1}s; {samples} samples ({during} while writing), {} violations; base {base}, final {last}, {} rows with a seq",
        wrote.as_secs_f64(),
        violations.len(),
        seqs.len()
    );
    assert!(
        violations.is_empty(),
        "violations: {:?}",
        &violations[..violations.len().min(10)]
    );
    assert!(samples >= 10_000);
    assert_eq!(last, base + total);
    // Unique and contiguous: exactly base+1 ..= base+total, once each.
    assert_eq!(seqs, ((base + 1)..=(base + total)).collect::<Vec<_>>());
    cleanup(&pg).await;
}

// ── I-24: the group-key upsert ───────────────────────────────────────────────

fn keyed(key: &str) -> NotifyMeta {
    NotifyMeta {
        group_key: Some(key.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn fifty_concurrent_writes_to_one_key_are_one_row() {
    let pg = wide_pool().await;
    cleanup(&pg).await;
    let ada = person(&pg, "group50").await;
    let base = notify_seq(&pg, &ada).await;
    let key = group_key::dm("5e000000-0000-4000-8000-0000000000d1");
    let cap = Captured::default();
    let mut tasks = Vec::new();
    for i in 0..50 {
        let deps = cap.deps(&pg);
        let ada = ada.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            let body = format!("message {i}");
            add_notification_with(
                &deps,
                &ada,
                &input("dm", "Bob", &body, "/comms/channel/x"),
                &keyed(&key),
            )
            .await
            .map(|f| (f.seq, body, f.inserted))
        }));
    }
    let mut filed = Vec::new();
    for t in tasks {
        filed.push(t.await.unwrap().expect("no write may fail (never 23505)"));
    }
    let (rows, count, body, seq): (i64, i32, String, i64) = sqlx::query_as(
        "select count(*) over (), count, body, seq from notifications where user_id = $1::uuid",
    )
    .bind(&ada)
    .fetch_one(&pg)
    .await
    .unwrap();
    let last = filed.iter().max_by_key(|(s, _, _)| *s).unwrap();
    println!(
        "group upsert: rows={rows} count={count} inserted={} body={body:?} (last writer {:?}), seqs {}..{}",
        filed.iter().filter(|f| f.2).count(),
        last.1,
        base + 1,
        notify_seq(&pg, &ada).await
    );
    assert_eq!(rows, 1);
    assert_eq!(count, 50);
    assert_eq!(
        filed.iter().filter(|f| f.2).count(),
        1,
        "exactly one insert"
    );
    assert_eq!(body, last.1, "the last writer's body");
    assert_eq!(seq, last.0);
    assert_eq!(notify_seq(&pg, &ada).await, base + 50, "50 seqs consumed");
    // Every write announced its seq.
    let frames = cap.on(&format!("user:{ada}"));
    assert_eq!(
        frames
            .iter()
            .filter(|f| f["type"] == "notification")
            .count(),
        50
    );
    cleanup(&pg).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn coalesce_quiet_realert_window_read_and_adoption() {
    let pg = pg().await;
    cleanup(&pg).await;
    let ada = person(&pg, "group").await;
    let deps = NotifyDeps::publishing(pg.clone(), None);
    let created = |id: String| {
        let pg = pg.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select (trunc(extract(epoch from created_at) * 1000))::bigint from notifications where id = $1::uuid",
            )
            .bind(id)
            .fetch_one(&pg)
            .await
            .unwrap()
        }
    };

    // Quiet keeps created_at; Realert moves it.
    let key = group_key::task_comments("t-1");
    let first = add_notification_with(
        &deps,
        &ada,
        &input("task-comment", "c", "one", "/boards/b/t-1"),
        &keyed(&key),
    )
    .await
    .unwrap();
    let t0 = created(first.id.clone()).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let quiet = NotifyMeta {
        coalesce: Coalesce::Quiet,
        ..keyed(&key)
    };
    let q = add_notification_with(
        &deps,
        &ada,
        &input("task-comment", "c", "two", "/boards/b/t-1"),
        &quiet,
    )
    .await
    .unwrap();
    assert_eq!(q.id, first.id);
    assert!(!q.inserted);
    assert_eq!(q.count, 2);
    assert_eq!(created(q.id.clone()).await, t0, "Quiet keeps created_at");
    tokio::time::sleep(Duration::from_millis(30)).await;
    let r = add_notification_with(
        &deps,
        &ada,
        &input("task-comment", "c", "three", "/boards/b/t-1"),
        &keyed(&key),
    )
    .await
    .unwrap();
    assert_eq!((r.id.clone(), r.count), (first.id.clone(), 3));
    assert!(created(r.id.clone()).await > t0, "Realert moves created_at");

    // Reading the row: the next write files fresh.
    mark_notifications_read(&pg, &ada, Some(std::slice::from_ref(&r.id)), None)
        .await
        .unwrap();
    let fresh = add_notification_with(
        &deps,
        &ada,
        &input("task-comment", "c", "four", "/boards/b/t-1"),
        &keyed(&key),
    )
    .await
    .unwrap();
    assert!(fresh.inserted);
    assert_ne!(fresh.id, r.id);

    // A 2 s window: after 3 s the open row keeps its content, loses its key,
    // and a second open row is filed.
    let wkey = group_key::channel_activity("c-1");
    let windowed = NotifyMeta {
        group_window_secs: Some(2),
        ..keyed(&wkey)
    };
    let a = add_notification_with(
        &deps,
        &ada,
        &input("channel-message", "#c", "early", "/comms/channel/c-1"),
        &windowed,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let b = add_notification_with(
        &deps,
        &ada,
        &input("channel-message", "#c", "late", "/comms/channel/c-1"),
        &windowed,
    )
    .await
    .unwrap();
    assert!(b.inserted);
    assert_ne!(a.id, b.id);
    let (old_key, old_body): (Option<String>, String) =
        sqlx::query_as("select group_key, body from notifications where id = $1::uuid")
            .bind(&a.id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(old_key, None, "the expired row left the group");
    assert_eq!(old_body, "early", "and kept its content");
    let open: i64 = sqlx::query_scalar(
        "select count(*) from notifications where user_id = $1::uuid and href = '/comms/channel/c-1' and read_at is null",
    )
    .bind(&ada)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(open, 2);

    // Adoption: a pre-contract DM row (no seq, no key) with a snooze on it.
    let chan = "5e000000-0000-4000-8000-0000000000d9";
    let href = format!("/comms/channel/{chan}");
    let (legacy,): (String,) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href) \
         values ($1::uuid, 'dm', 'Bob', 'a wave-1 row', $2) returning id::text",
    )
    .bind(&ada)
    .bind(&href)
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into inbox_focus_state (user_id, source_type, source_id, snoozed_until) \
         values ($1::uuid, 'notification', $2, now() + interval '3 days')",
    )
    .bind(&ada)
    .bind(&legacy)
    .execute(&pg)
    .await
    .unwrap();
    let adopt = NotifyMeta {
        adopt_legacy: true,
        ..keyed(&group_key::dm(chan))
    };
    let adopted = add_notification_with(
        &deps,
        &ada,
        &input("dm", "Bob", "the next DM", &href),
        &adopt,
    )
    .await
    .unwrap();
    assert_eq!(adopted.id, legacy, "adoption keeps the id");
    assert!(!adopted.inserted);
    assert_eq!(adopted.count, 2);
    let snoozed: bool = sqlx::query_scalar(
        "select snoozed_until > now() from inbox_focus_state \
         where user_id = $1::uuid and source_type = 'notification' and source_id = $2",
    )
    .bind(&ada)
    .bind(&legacy)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(snoozed, "its snooze stayed with it");
    println!(
        "coalesce: quiet kept {t0}, realert moved it; window split a/b; adopted {legacy} with its snooze"
    );
    cleanup(&pg).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn concurrent_window_expiry_never_conflicts() {
    let pg = wide_pool().await;
    cleanup(&pg).await;
    let ada = person(&pg, "expiry").await;
    let key = group_key::approval("a-1");
    let meta = NotifyMeta {
        group_window_secs: Some(1),
        adopt_legacy: true,
        ..keyed(&key)
    };
    // Four bursts of 10 concurrent writers, 1.5 s apart: the window lapses
    // between bursts, so in each burst the first writer to take the person's
    // row lock expires the open row while nine others race it to the upsert.
    let mut tasks = Vec::new();
    for burst in 0..4u64 {
        for i in 0..10u64 {
            let deps = NotifyDeps::publishing(pg.clone(), None);
            let ada = ada.clone();
            let meta = meta.clone();
            tasks.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(burst * 1_500 + i)).await;
                add_notification_with(
                    &deps,
                    &ada,
                    &input("approval_pending", "Approve?", "x", "/approvals/a-1"),
                    &meta,
                )
                .await
            }));
        }
    }
    let mut errors = Vec::new();
    for t in tasks {
        if let Err(e) = t.await.unwrap() {
            errors.push(e.to_string());
        }
    }
    let (rows, open): (i64, i64) = sqlx::query_as(
        "select count(*), count(*) filter (where group_key is not null) from notifications where user_id = $1::uuid",
    )
    .bind(&ada)
    .fetch_one(&pg)
    .await
    .unwrap();
    println!(
        "expiry: 4 bursts x 10 writes → {rows} rows, {open} still keyed, {} errors",
        errors.len()
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(open, 1, "one open row holds the key");
    assert_eq!(rows, 4, "the window split the group once per burst");
    cleanup(&pg).await;
}

// ── I-24: entity clears; I-18: the ack ──────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn entity_clears_and_the_ack() {
    let pg = pg().await;
    cleanup(&pg).await;
    let ada = person(&pg, "ent-ada").await;
    let bob = person(&pg, "ent-bob").await;
    let cap = Captured::default();
    let deps = cap.deps(&pg);
    let x = NotifyEntity::new(entity::APPROVAL, "approval-x");
    let about = NotifyMeta {
        entity: Some(x.clone()),
        ..Default::default()
    };
    let mut ids = Vec::new();
    for who in [&ada, &ada, &bob] {
        ids.push(
            add_notification_with(
                &deps,
                who,
                &input("approval_pending", "Approve?", "x", "/approvals/x"),
                &about,
            )
            .await
            .unwrap()
            .id,
        );
    }
    // clear_entity: only the caller's.
    let mine = clear_entity(&deps, &bob, &x).await.unwrap();
    assert_eq!(mine, vec![ids[2].clone()]);
    let ada_unread: i64 = sqlx::query_scalar(
        "select count(*) from notifications where user_id = $1::uuid and read_at is null",
    )
    .bind(&ada)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(ada_unread, 2, "clear_entity touched nobody else");
    // resolve_entity: every recipient, each told.
    let bob2 = add_notification_with(
        &deps,
        &bob,
        &input("approval_pending", "Approve?", "x", "/approvals/x"),
        &about,
    )
    .await
    .unwrap();
    let n = resolve_entity(&deps, &x).await.unwrap();
    assert_eq!(n, 3);
    for (who, want) in [
        (&ada, vec![ids[0].clone(), ids[1].clone()]),
        (&bob, vec![bob2.id.clone()]),
    ] {
        let reads: Vec<_> = cap
            .on(&format!("user:{who}"))
            .into_iter()
            .filter(|f| f["type"] == "notifications")
            .collect();
        let last = reads.last().expect("a read-sync frame");
        let mut got: Vec<String> = last["readIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        got.sort();
        let mut want = want;
        want.sort();
        assert_eq!(got, want);
        assert_eq!(last["all"], false);
        assert_eq!(last["unread"], 0);
    }
    // The ack: the caller's own row only, first ack wins.
    let ack1 = defer::ack(&pg, &ada, std::slice::from_ref(&ids[0]))
        .await
        .unwrap();
    let ack_foreign = defer::ack(&pg, &bob, std::slice::from_ref(&ids[0]))
        .await
        .unwrap();
    assert_eq!((ack1, ack_foreign), (1, 0));
    let surfaced: bool =
        sqlx::query_scalar("select surfaced_at is not null from notifications where id = $1::uuid")
            .bind(&ids[0])
            .fetch_one(&pg)
            .await
            .unwrap();
    assert!(surfaced);
    println!(
        "entity: clear_entity 1 (bob only), resolve_entity {n} across 2 people; ack 1 / foreign 0"
    );
    cleanup(&pg).await;
}

// ── C32: the write path's cost, before and after ────────────────────────────

fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

#[tokio::test]
#[ignore = "needs the C32 scratch database (C32_DATABASE_URL)"]
async fn write_path_timing() {
    let Ok(url) = std::env::var("C32_DATABASE_URL") else {
        println!("write_path_timing: C32_DATABASE_URL unset — skipped");
        return;
    };
    let pg = PgPool::connect(&url).await.unwrap();
    let users: Vec<String> = sqlx::query_scalar(
        "select id::text from users where sub like 'c32-%' order by sub limit 50",
    )
    .fetch_all(&pg)
    .await
    .unwrap();
    assert!(!users.is_empty(), "load the c32 volume first");
    let deps = NotifyDeps::publishing(pg.clone(), None);
    const N: usize = 1_000;
    // Wave 1's insert-only path, statement for statement: the prefs read,
    // then the plain insert (`add_notification` at 48cde72f).
    let mut before = Vec::with_capacity(N);
    for i in 0..N {
        let user = &users[i % users.len()];
        let t = Instant::now();
        let _: Option<serde_json::Value> =
            sqlx::query_scalar("select notify_prefs from users where id = $1::uuid")
                .bind(user)
                .fetch_optional(&pg)
                .await
                .unwrap()
                .flatten();
        let _: (String, i64) = sqlx::query_as(
            "insert into notifications (user_id, kind, title, body, href, read_at) \
             values ($1::uuid, $2, $3, $4, $5, null) \
             returning id::text, (trunc(extract(epoch from created_at) * 1000))::bigint",
        )
        .bind(user)
        .bind("dm")
        .bind("timing (wave 1)")
        .bind("the wave-1 insert-only path")
        .bind("/comms/dm/x")
        .fetch_one(&pg)
        .await
        .unwrap();
        before.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    before.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // C32_ONLY=before: the schema has no contract columns yet (the wave-1
    // baseline run, before the DDL).
    if std::env::var("C32_ONLY").as_deref() == Ok("before") {
        println!(
            "write path over {N} sequential writes, {} users: wave-1 schema, wave-1 path p50 {:.2} ms p95 {:.2} ms",
            users.len(),
            pct(&before, 0.5),
            pct(&before, 0.95)
        );
        sqlx::query("delete from notifications where title like 'timing (%'")
            .execute(&pg)
            .await
            .unwrap();
        return;
    }
    // The contract's path: add_notification (no key) and, beside it, a keyed
    // coalescing write — the one W2-P's producers will make.
    let mut after = Vec::with_capacity(N);
    for i in 0..N {
        let user = &users[i % users.len()];
        let t = Instant::now();
        add_notification(
            &deps,
            user,
            &input(
                "dm",
                "timing (contract)",
                "the contract path",
                "/comms/dm/x",
            ),
        )
        .await
        .unwrap();
        after.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let mut keyed_ms = Vec::with_capacity(N);
    for i in 0..N {
        let user = &users[i % users.len()];
        let meta = NotifyMeta {
            group_key: Some(group_key::dm(&format!("timing-{}", i % 10))),
            group_window_secs: Some(3600),
            adopt_legacy: true,
            ..Default::default()
        };
        let t = Instant::now();
        add_notification_with(
            &deps,
            user,
            &input("dm", "timing (keyed)", "a coalescing write", "/comms/dm/x"),
            &meta,
        )
        .await
        .unwrap();
        keyed_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    for v in [&mut after, &mut keyed_ms] {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    println!(
        "write path over {N} sequential writes, {} users: wave-1 p50 {:.2} ms p95 {:.2} ms | contract p50 {:.2} ms p95 {:.2} ms | keyed p50 {:.2} ms p95 {:.2} ms",
        users.len(),
        pct(&before, 0.5),
        pct(&before, 0.95),
        pct(&after, 0.5),
        pct(&after, 0.95),
        pct(&keyed_ms, 0.5),
        pct(&keyed_ms, 0.95)
    );
    // Leave c32 as loaded (it is dropped after the measurement).
    sqlx::query("delete from notifications where title like 'timing (%'")
        .execute(&pg)
        .await
        .unwrap();
}

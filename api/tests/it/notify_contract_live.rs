// Live-DB proof of the notification contract's write path (cargo test --
// --ignored): the per-user, commit-ordered seq under concurrent writers
// (I-10), the group-key upsert and its window, adoption of a pre-contract
// row, the entity clears, and the ack. Postgres decides every one of these —
// the row lock, the partial unique index, ON CONFLICT — so only a real
// database can confirm them. #[ignore]d, so the plain `cargo test` never
// needs a database; ci.yml's api job runs them with --ignored (its live-DB
// steps).
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
    refresh_notification, reopen_notifications, resolve_entity,
};
use talaria_api::realtime::RealtimeDeps;

const IN_APP: &str = r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#;

/// One test at a time, even when the harness runs a module's tests in
/// parallel (CI does): the seq test commits 16,000 rows, and a coalescing
/// test sharing the database with it measures that load, not its own rule.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// Delete this test's own throwaway people (the cascade takes their rows).
/// Scoped by tag: CI runs a module's tests concurrently, so one test's
/// cleanup must never reach another's people.
async fn cleanup(pg: &PgPool, tags: &[&str]) {
    let emails: Vec<String> = tags
        .iter()
        .map(|t| format!("notify-contract-{t}@test.invalid"))
        .collect();
    sqlx::query("delete from users where email = any($1)")
        .bind(&emails)
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
        .max_connections(12)
        .acquire_timeout(Duration::from_secs(120))
        .connect(&url)
        .await
        .unwrap()
}

// ── I-10: seq order IS commit order ─────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn seq_commit_order() {
    let _serial = SERIAL.lock().await;
    let pg = wide_pool().await;
    cleanup(&pg, &["seq"]).await;
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
    cleanup(&pg, &["seq"]).await;
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
    let _serial = SERIAL.lock().await;
    let pg = wide_pool().await;
    cleanup(&pg, &["group50"]).await;
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
            .map(|f| (f.seq, body, f.inserted, f.created_ms))
        }));
    }
    let mut filed = Vec::new();
    for t in tasks {
        filed.push(t.await.unwrap().expect("no write may fail (never 23505)"));
    }
    // Every Realert moves createdAt strictly LATER, in seq (= commit) order —
    // the re-alert rule every client keeps (a known id whose createdAt moved
    // later). A writer that queued behind the person's row lock must not
    // stamp the transaction's start, which can be earlier than the commit it
    // waited for.
    let mut by_seq: Vec<(i64, i64)> = filed.iter().map(|f| (f.0, f.3)).collect();
    by_seq.sort();
    let backwards: Vec<_> = by_seq
        .windows(2)
        .filter(|w| w[1].1 <= w[0].1)
        .map(|w| (w[0], w[1]))
        .collect();
    assert!(
        backwards.is_empty(),
        "createdAt did not rise with seq ({} of 49 steps): {:?}",
        backwards.len(),
        &backwards[..backwards.len().min(5)]
    );
    let (rows, count, body, seq): (i64, i32, String, i64) = sqlx::query_as(
        "select count(*) over (), count, body, seq from notifications where user_id = $1::uuid",
    )
    .bind(&ada)
    .fetch_one(&pg)
    .await
    .unwrap();
    let last = filed.iter().max_by_key(|f| f.0).unwrap();
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
    cleanup(&pg, &["group50"]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn coalesce_quiet_realert_window_read_and_adoption() {
    let _serial = SERIAL.lock().await;
    let pg = pg().await;
    cleanup(&pg, &["group"]).await;
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
    cleanup(&pg, &["group"]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn concurrent_window_expiry_never_conflicts() {
    let _serial = SERIAL.lock().await;
    let pg = wide_pool().await;
    let side = crate::support::pg().await;
    cleanup(&pg, &["expiry"]).await;
    let ada = person(&pg, "expiry").await;
    let key = group_key::approval("a-1");
    // THE WINDOW LAPSES BY THE CLOCK, NOT BY A SLEEP. Bursts used to start
    // 2.5 s apart on a 1 s window, timed from the spawn: a burst whose commits
    // a stalled disk held past the next burst's start touched the row inside
    // that burst's window, and the next burst rightly joined it (2 rows of 4
    // on a host under IO pressure). Now each burst lands before the next
    // begins, and the person's rows are moved back past a window no slow
    // writer can outlast, so every burst meets an expired row and only that.
    const WINDOW: u32 = 600;
    let meta = NotifyMeta {
        group_window_secs: Some(WINDOW),
        adopt_legacy: true,
        ..keyed(&key)
    };
    let mut errors = Vec::new();
    for burst in 1..=4i64 {
        if burst > 1 {
            sqlx::query(
                "update notifications \
                 set created_at = created_at - $2::int * interval '1 second', \
                     updated_at = updated_at - $2::int * interval '1 second' \
                 where user_id = $1::uuid",
            )
            .bind(&ada)
            .bind(WINDOW as i32 + 60)
            .execute(&pg)
            .await
            .unwrap();
        }
        // Ten writers, all queued behind the person's row lock before the
        // first one holds it: the first expires the open row and inserts,
        // and the nine behind it race that insert to the upsert — every run,
        // not only when the scheduler happens to line them up.
        let mut gate = side.begin().await.unwrap();
        sqlx::query("select 1 from users where id = $1::uuid for update")
            .bind(&ada)
            .execute(&mut *gate)
            .await
            .unwrap();
        let tasks: Vec<_> = (0..10)
            .map(|_| {
                let deps = NotifyDeps::publishing(pg.clone(), None);
                let ada = ada.clone();
                let meta = meta.clone();
                tokio::spawn(async move {
                    add_notification_with(
                        &deps,
                        &ada,
                        &input("approval_pending", "Approve?", "x", "/approvals/a-1"),
                        &meta,
                    )
                    .await
                })
            })
            .collect();
        let queued = Instant::now();
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "select count(*) from pg_stat_activity \
                 where datname = current_database() and wait_event_type = 'Lock' \
                   and query like 'update users set notify_seq%'",
            )
            .fetch_one(&side)
            .await
            .unwrap();
            if waiting >= 10 {
                break;
            }
            assert!(
                queued.elapsed() < Duration::from_secs(60),
                "burst {burst}: {waiting} of 10 writers queued on the lock after 60 s"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        gate.commit().await.unwrap();
        for t in tasks {
            if let Err(e) = t.await.unwrap() {
                errors.push(format!("burst {burst}: {e}"));
            }
        }
        let (rows, open, count): (i64, i64, Option<i32>) = sqlx::query_as(
            "select count(*), count(*) filter (where group_key is not null), \
                    max(count) filter (where group_key is not null) \
             from notifications where user_id = $1::uuid",
        )
        .bind(&ada)
        .fetch_one(&pg)
        .await
        .unwrap();
        println!(
            "expiry: burst {burst} x 10 writes → {rows} rows, {open} keyed of count {count:?}"
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(open, 1, "burst {burst}: one open row holds the key");
        assert_eq!(count, Some(10), "burst {burst}: its ten writes are one row");
        assert_eq!(
            rows, burst,
            "burst {burst}: the window split the group once per burst"
        );
    }
    cleanup(&pg, &["expiry"]).await;
}

// ── I-24: a key is one person's; a read row comes back without a fight ─────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn keys_are_per_person_and_a_reopen_never_conflicts() {
    let _serial = SERIAL.lock().await;
    let pg = pg().await;
    cleanup(&pg, &["iso-ada", "iso-bob"]).await;
    let ada = person(&pg, "iso-ada").await;
    let bob = person(&pg, "iso-bob").await;
    let deps = NotifyDeps::publishing(pg.clone(), None);
    let key = group_key::dm("5e000000-0000-4000-8000-0000000000e1");
    let href = "/comms/channel/5e000000-0000-4000-8000-0000000000e1";

    // One key, two people: two rows, each its own count — never one row.
    let a1 = add_notification_with(&deps, &ada, &input("dm", "Cy", "1", href), &keyed(&key))
        .await
        .unwrap();
    let b1 = add_notification_with(&deps, &bob, &input("dm", "Cy", "1", href), &keyed(&key))
        .await
        .unwrap();
    let a2 = add_notification_with(&deps, &ada, &input("dm", "Cy", "2", href), &keyed(&key))
        .await
        .unwrap();
    assert!(
        a1.inserted && b1.inserted,
        "each person files their own row"
    );
    assert_ne!(a1.id, b1.id);
    assert_eq!(
        (a2.id.clone(), a2.count),
        (a1.id.clone(), 2),
        "Ada's coalesces into Ada's"
    );
    let bob_count: i32 = sqlx::query_scalar("select count from notifications where id = $1::uuid")
        .bind(&b1.id)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(bob_count, 1, "Bob's row never saw Ada's write");
    // A window expiry is one person's too: Ada's 0 s window splits only Ada's.
    let split = NotifyMeta {
        group_window_secs: Some(0),
        ..keyed(&key)
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    let a3 = add_notification_with(&deps, &ada, &input("dm", "Cy", "3", href), &split)
        .await
        .unwrap();
    assert!(a3.inserted);
    let bob_key: Option<String> =
        sqlx::query_scalar("select group_key from notifications where id = $1::uuid")
            .bind(&b1.id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(
        bob_key.as_deref(),
        Some(key.as_str()),
        "Bob's row kept its key"
    );

    // The reopen: Ada reads a3, the group files a4 (fresh, keyed), then the
    // Home Inbox's undo brings a3 back. Before the reopen door that was a
    // second open row for (Ada, key): 23505.
    let ids = mark_notifications_read(&pg, &ada, Some(std::slice::from_ref(&a3.id)), None)
        .await
        .unwrap();
    assert_eq!(ids, vec![a3.id.clone()]);
    // What the Home Inbox records as `afterReadAt`: the read at ms precision.
    let read_ms: i64 = sqlx::query_scalar(
        "select (trunc(extract(epoch from read_at) * 1000))::bigint from notifications where id = $1::uuid",
    )
    .bind(&a3.id)
    .fetch_one(&pg)
    .await
    .unwrap();
    let read_iso = talaria_api::agent_auth::epoch_ms_to_iso(read_ms);
    // The guard wave 1's undo used, an exact compare against that ms string,
    // matches no microsecond-stamped read (the undo could never restore).
    let (exact, whole_ms): (i64, bool) = sqlx::query_as(
        "select count(*) filter (where read_at = $2::timestamptz), \
                bool_and(read_at = date_trunc('milliseconds', read_at)) \
         from notifications where id = $1::uuid",
    )
    .bind(&a3.id)
    .bind(&read_iso)
    .fetch_one(&pg)
    .await
    .unwrap();
    let a4 = add_notification_with(&deps, &ada, &input("dm", "Cy", "4", href), &keyed(&key))
        .await
        .unwrap();
    assert!(a4.inserted, "the read row left the group");
    // A stale guard (a newer read) reopens nothing.
    let stale = reopen_notifications(
        &pg,
        &ada,
        std::slice::from_ref(&a3.id),
        Some("2001-01-01T00:00:00.000Z"),
    )
    .await
    .unwrap();
    assert!(stale.is_empty());
    let back = reopen_notifications(&pg, &ada, std::slice::from_ref(&a3.id), Some(&read_iso))
        .await
        .expect("a reopen never fails the open-group index");
    assert_eq!(back, vec![a3.id.clone()]);
    let (a3_read, a3_key): (bool, Option<String>) =
        sqlx::query_as("select read_at is null, group_key from notifications where id = $1::uuid")
            .bind(&a3.id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert!(a3_read, "unread again");
    assert_eq!(a3_key, None, "and it gave the key up to the open a4");
    let a5 = add_notification_with(&deps, &ada, &input("dm", "Cy", "5", href), &keyed(&key))
        .await
        .unwrap();
    assert_eq!(
        (a5.id.clone(), a5.inserted),
        (a4.id.clone(), false),
        "the group writes into a4"
    );
    // With no open holder the key comes back with the row; two reopened rows
    // of one group in one call keep it on the newest only.
    let both = mark_notifications_read(&pg, &ada, Some(std::slice::from_ref(&a4.id)), None)
        .await
        .unwrap();
    assert_eq!(both, vec![a4.id.clone()]);
    sqlx::query("update notifications set group_key = $2, read_at = now() where id = $1::uuid")
        .bind(&a3.id)
        .bind(&key)
        .execute(&pg)
        .await
        .unwrap();
    let pair = reopen_notifications(&pg, &ada, &[a3.id.clone(), a4.id.clone()], None)
        .await
        .expect("two rows of one group reopened together never conflict");
    assert_eq!(pair.len(), 2);
    let holders: Vec<String> = sqlx::query_scalar(
        "select id::text from notifications where user_id = $1::uuid and group_key = $2 and read_at is null",
    )
    .bind(&ada)
    .bind(&key)
    .fetch_all(&pg)
    .await
    .unwrap();
    assert_eq!(holders.len(), 1, "one open row holds the key");

    // refresh_notification: same id, a seq of its own, createdAt strictly later.
    let before_seq = notify_seq(&pg, &ada).await;
    let before_ms: i64 = sqlx::query_scalar(
        "select (trunc(extract(epoch from created_at) * 1000))::bigint from notifications where id = $1::uuid",
    )
    .bind(&b1.id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        refresh_notification(&deps, &b1.id, &bob, &input("dm", "Cy", "again", href))
            .await
            .unwrap()
    );
    let (seq, ms): (Option<i64>, i64) = sqlx::query_as(
        "select seq, (trunc(extract(epoch from created_at) * 1000))::bigint from notifications where id = $1::uuid",
    )
    .bind(&b1.id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        seq,
        Some(notify_seq(&pg, &bob).await),
        "the refresh took Bob's next seq"
    );
    assert!(ms > before_ms, "and moved createdAt later");
    assert_eq!(
        notify_seq(&pg, &ada).await,
        before_seq,
        "Ada's cursor never moved for Bob"
    );
    println!(
        "isolation: one key, 2 people → 2 rows; reopen with a newer open holder kept 1 holder; the wave-1 exact undo guard matched {exact}"
    );
    assert!(
        exact == 0 || whole_ms,
        "wave 1's exact guard never matches a read stamped past the millisecond"
    );
    cleanup(&pg, &["iso-ada", "iso-bob"]).await;
}

// ── I-24: entity clears; I-18: the ack ──────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn entity_clears_and_the_ack() {
    let _serial = SERIAL.lock().await;
    let pg = pg().await;
    cleanup(&pg, &["ent-ada", "ent-bob"]).await;
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
    cleanup(&pg, &["ent-ada", "ent-bob"]).await;
}

// ── C32: the write path's cost, before and after ────────────────────────────

fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

#[tokio::test]
#[ignore = "needs the C32 scratch database (C32_DATABASE_URL)"]
async fn write_path_timing() {
    let _serial = SERIAL.lock().await;
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

// Live-DB proof of the push plane's row tending and keypair custody
// (cargo test -- --ignored). The cryptography is pinned to RFC vectors in
// the unit tests; what only Postgres can confirm is the LOOP — which rows
// a delivery keeps, touches, counts, or prunes, and that the instance's
// VAPID keypair is born exactly once even when several first deliveries
// race. House rule: #[ignore]d, never CI.
//
// SAFE ON A SHARED DEV DATABASE: the instance's own `push_vapid` row is
// SNAPSHOTTED before a test touches it and RESTORED after, pass or fail.
// The suite used to delete it, which rotated the instance key under every
// subscription on whatever database it ran against.
//
//   DATABASE_URL=postgres://… cargo test --test it push_live:: -- --ignored

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::support::pg;
use base64::Engine as _;
use futures_util::FutureExt as _;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use sqlx::postgres::PgPool;
use talaria_api::notify::{NotificationInput, NotifyDeps, add_notification};
use talaria_api::push::{
    PostPushFn, PushAnswer, PushFailure, PushNote, PushPolicy, PushPost, PushResult, VAPID_KEY,
    deliver_push_with, vapid_keys,
};
use talaria_api::secretbox::SecretBox;

const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The suite's instance-row lock. Both tests BIRTH and DELETE the one
/// app_settings keypair row (the race test has no other subject), and cargo
/// runs them concurrently by default — one test's cleanup landing between
/// the other's insert and its re-read is a spurious "the vapid keypair row
/// vanished", a flake about scheduling, not about the plane.
static KEYPAIR_ROW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// The suite's throwaway people: the user cascade takes push_subscriptions
/// with it.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'push-live-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

/// Run `body` with the instance keypair row cleared, then put the original
/// back — pass OR fail (a panic is caught, the row restored, the panic
/// resumed). The row is the instance's identity to every push service; a
/// test that left it gone or replaced stranded every real subscription.
async fn with_vapid_row_set_aside<F>(pg: &PgPool, body: F)
where
    F: std::future::Future<Output = ()>,
{
    let saved: Option<serde_json::Value> =
        sqlx::query_scalar("select value from app_settings where key = $1")
            .bind(VAPID_KEY)
            .fetch_optional(pg)
            .await
            .unwrap();
    sqlx::query("delete from app_settings where key = $1")
        .bind(VAPID_KEY)
        .execute(pg)
        .await
        .unwrap();
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    sqlx::query("delete from app_settings where key = $1")
        .bind(VAPID_KEY)
        .execute(pg)
        .await
        .unwrap();
    if let Some(value) = saved {
        sqlx::query("insert into app_settings (key, value) values ($1, $2)")
            .bind(VAPID_KEY)
            .bind(value)
            .execute(pg)
            .await
            .unwrap();
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Short spacing so the retry paths run in milliseconds; the subject is the
/// one an https public URL produces.
fn quick() -> PushPolicy {
    PushPolicy {
        backoff: [Duration::from_millis(20), Duration::from_millis(40)],
        retry_after_cap: Duration::from_secs(60),
    }
}

fn note(id: &str) -> PushNote {
    PushNote {
        id: id.into(),
        title: "Priya says".into(),
        body: "while you were away".into(),
        href: "/comms/agent/claude-test/c1".into(),
        kind: "agent-reply".into(),
        renotify: false,
        created_at: "2026-10-31T12:00:00.000Z".into(),
        ..Default::default()
    }
}

async fn row_state(
    pg: &PgPool,
    endpoint: &str,
) -> Option<(i32, Option<i32>, Option<String>, bool)> {
    sqlx::query_as(
        "select failures, last_status, last_error, last_attempt_at is not null \
         from push_subscriptions where endpoint = $1",
    )
    .bind(endpoint)
    .fetch_optional(pg)
    .await
    .unwrap()
}

async fn person(pg: &PgPool, suffix: &str) -> String {
    // sub and email both carry the suffix: the cleanup's `push-live-%`
    // pattern must match them, or a failed run's leftover user blocks the
    // next one's insert forever.
    let sub = format!("push-live-{suffix}");
    let email = format!("push-live-{suffix}@test.invalid");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) \
         values ($1, $2, 'Push Live', 'member') \
         returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// A secretbox that can seal: v2 tokens need an ACTIVE DEK, so the test box
/// carries one version. Not the instance key — proving the seal is real is
/// part of the point.
fn test_box() -> SecretBox {
    SecretBox::from_parts([7u8; 32], HashMap::from([(1u32, [9u8; 32])]), Some(1))
}

/// One subscription row with FRESH, legal key material (a real P-256 point
/// and a 16-byte auth secret), optionally backdated so a liveness touch is
/// observable.
async fn subscription(pg: &PgPool, user: &str, endpoint: &str, backdate: bool) {
    let secret = p256::SecretKey::generate();
    let point = secret.public_key().to_sec1_point(false);
    let p256dh = B64U.encode(point.as_bytes());
    let auth = B64U.encode([3u8; 16]);
    let sql = if backdate {
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth, last_seen_at) \
         values ($1::uuid, $2, $3, $4, now() - interval '1 hour')"
    } else {
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth) \
         values ($1::uuid, $2, $3, $4)"
    };
    sqlx::query(sql)
        .bind(user)
        .bind(endpoint)
        .bind(&p256dh)
        .bind(&auth)
        .execute(pg)
        .await
        .unwrap();
}

async fn endpoint_exists(pg: &PgPool, endpoint: &str) -> bool {
    sqlx::query_scalar::<_, i64>("select count(*) from push_subscriptions where endpoint = $1")
        .bind(endpoint)
        .fetch_one(pg)
        .await
        .unwrap()
        > 0
}

/// The fake send edge: every delivery is captured, and the answer is chosen
/// by the endpoint's last path segment — a, b, c, d — so one call walks
/// every branch of the tending match.
fn fake_post(captured: Arc<Mutex<Vec<PushPost>>>) -> PostPushFn {
    Arc::new(move |post: PushPost| {
        let captured = captured.clone();
        let code: u16 = match post.endpoint.rsplit('/').next().unwrap_or("") {
            "a" => 200,
            "b" => 201,
            "c" => 410,
            "d" => 500,
            _ => 200,
        };
        Box::pin(async move {
            captured.lock().unwrap().push(post);
            Ok(PushAnswer::status(code))
        })
    })
}

/// A scripted edge: each endpoint answers from its own queue, one answer per
/// POST (the last answer repeats), and every POST is captured with its time.
type Script = HashMap<String, Vec<PushResult>>;
fn scripted(script: Script, captured: Arc<Mutex<Vec<(String, Instant, usize)>>>) -> PostPushFn {
    let script = Arc::new(Mutex::new(script));
    Arc::new(move |post: PushPost| {
        let script = script.clone();
        let captured = captured.clone();
        Box::pin(async move {
            let answer = {
                let mut s = script.lock().unwrap();
                let q = s.get_mut(&post.endpoint).expect("a scripted endpoint");
                if q.len() > 1 {
                    q.remove(0)
                } else {
                    q[0].clone()
                }
            };
            captured
                .lock()
                .unwrap()
                .push((post.endpoint.clone(), Instant::now(), post.body.len()));
            if post.endpoint.ends_with("/hang") {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            answer
        })
    })
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn delivery_touches_prunes_and_keeps_by_the_services_answer() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    with_vapid_row_set_aside(&pg, delivery_body(&pg)).await;
}

async fn delivery_body(pg: &PgPool) {
    let pg = pg.clone();
    cleanup(&pg).await;
    let user = person(&pg, "delivery").await;
    let sb = test_box();

    subscription(&pg, &user, "https://push.test/a", true).await; // 2xx, backdated
    subscription(&pg, &user, "https://push.test/b", false).await; // also 2xx
    subscription(&pg, &user, "https://push.test/c", false).await; // 410
    subscription(&pg, &user, "https://push.test/d", false).await; // 500
    // Undecodable key material: dead weight, pruned before any POST.
    sqlx::query(
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth) \
         values ($1::uuid, 'https://push.test/e', '!!not base64!!', 'whatever')",
    )
    .bind(&user)
    .execute(&pg)
    .await
    .unwrap();

    let captured: Arc<Mutex<Vec<PushPost>>> = Arc::new(Mutex::new(Vec::new()));
    let post = fake_post(captured.clone());
    deliver_push_with(
        &pg,
        &sb,
        &post,
        &user,
        &note("note-1"),
        &quick(),
        "https://talaria.example.test",
    )
    .await;

    // Every LEGAL row was posted; the garbage row never was. The 500 (d) is
    // retried twice — three POSTs — and every other answer is final.
    let posts = captured.lock().unwrap();
    let mut posted: Vec<&str> = posts.iter().map(|p| p.endpoint.as_str()).collect();
    posted.sort_unstable();
    assert_eq!(
        posted,
        [
            "https://push.test/a",
            "https://push.test/b",
            "https://push.test/c",
            "https://push.test/d",
            "https://push.test/d",
            "https://push.test/d"
        ]
    );

    // What left the building is a well-formed aes128gcm body under a VAPID
    // Authorization — the header's anatomy, not the ciphertext's bytes
    // (those are the RFC vector's job in the unit tests).
    for p in posts.iter() {
        assert!(p.body.len() > 16 + 4 + 1 + 65);
        assert_eq!(&p.body[16..20], &4096u32.to_be_bytes());
        assert_eq!(p.body[20], 65);
        assert_eq!(p.body[21], 0x04); // uncompressed point
        assert!(p.authorization.starts_with("vapid t="));
        assert!(p.authorization.contains(", k="));
        // The subject handed in is the JWT's `sub`.
        let jwt = p.authorization["vapid t=".len()..]
            .split(',')
            .next()
            .unwrap();
        let claims = B64U.decode(jwt.split('.').nth(1).unwrap()).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&claims).unwrap();
        assert_eq!(claims["sub"], "https://talaria.example.test");
    }
    drop(posts);

    // Row tending: a and b SURVIVE (touched), c is GONE (the service forgot
    // it), d SURVIVES (a 500 is the service's bad minute, not the row's
    // death), and the garbage row is gone without ever being posted.
    assert!(endpoint_exists(&pg, "https://push.test/a").await);
    assert!(endpoint_exists(&pg, "https://push.test/b").await);
    assert!(!endpoint_exists(&pg, "https://push.test/c").await);
    assert!(endpoint_exists(&pg, "https://push.test/d").await);
    assert!(!endpoint_exists(&pg, "https://push.test/e").await);

    // The 2xx touch: a's last_seen_at moved off the hour-old backdate.
    let (seen_a,): (f64,) = sqlx::query_as(
        "select extract(epoch from (now() - last_seen_at))::float8 from push_subscriptions \
         where endpoint = 'https://push.test/a'",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        seen_a < 300.0,
        "the 2xx touch should be seconds old, was {seen_a}"
    );
    // Every attempt is on the row: the 2xx reset and recorded, the 500
    // counted once (one delivery, however many POSTs it took).
    let a = row_state(&pg, "https://push.test/a").await.unwrap();
    assert_eq!((a.0, a.1, a.3), (0, Some(200), true));
    let d = row_state(&pg, "https://push.test/d").await.unwrap();
    assert_eq!((d.0, d.1, d.3), (1, Some(500), true));
    assert!(d.2.unwrap().starts_with("500"));

    cleanup(&pg).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn refusals_prune_at_three_and_the_instances_own_fault_is_not_counted() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    let body = async {
        cleanup(&pg).await;
        let user = person(&pg, "refusals").await;
        let sb = test_box();
        subscription(&pg, &user, "https://push.test/refused", false).await;
        subscription(&pg, &user, "https://push.test/jwt", false).await;
        subscription(&pg, &user, "https://push.test/gone", false).await;
        let mut script = Script::new();
        script.insert(
            "https://push.test/refused".into(),
            vec![Ok(PushAnswer::status(403))],
        );
        script.insert(
            "https://push.test/jwt".into(),
            vec![Ok(PushAnswer {
                status: 403,
                retry_after: None,
                body: "{\"reason\":\"BadJwtToken\"}".into(),
            })],
        );
        script.insert(
            "https://push.test/gone".into(),
            vec![Ok(PushAnswer::status(404))],
        );
        let captured = Arc::new(Mutex::new(Vec::new()));
        let post = scripted(script, captured.clone());
        for i in 1..=3 {
            deliver_push_with(
                &pg,
                &sb,
                &post,
                &user,
                &note(&format!("n{i}")),
                &quick(),
                "mailto:x@y",
            )
            .await;
            // 404 prunes at once, on the first delivery.
            assert!(!endpoint_exists(&pg, "https://push.test/gone").await);
            let jwt = row_state(&pg, "https://push.test/jwt").await.unwrap();
            assert_eq!(jwt.0, 0, "an instance JWT fault is never the device's");
            assert_eq!(jwt.1, Some(403));
            if i < 3 {
                let r = row_state(&pg, "https://push.test/refused").await.unwrap();
                assert_eq!(r.0, i, "a refusal counts once per delivery");
            }
        }
        // The third consecutive refusal prunes.
        assert!(!endpoint_exists(&pg, "https://push.test/refused").await);
        assert!(endpoint_exists(&pg, "https://push.test/jwt").await);
        cleanup(&pg).await;
    };
    with_vapid_row_set_aside(&pg, body).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn throttles_oversize_and_hangs_settle_per_device() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    let body = async {
        cleanup(&pg).await;
        let user = person(&pg, "settle").await;
        let sb = test_box();
        for e in ["throttled", "toolarge", "hang", "ok"] {
            subscription(&pg, &user, &format!("https://push.test/{e}"), false).await;
        }
        let mut script = Script::new();
        // 429 with Retry-After 0, then accepted: retried once, delivered.
        script.insert(
            "https://push.test/throttled".into(),
            vec![
                Ok(PushAnswer {
                    status: 429,
                    retry_after: Some(0),
                    body: String::new(),
                }),
                Ok(PushAnswer::status(201)),
            ],
        );
        // 413, then accepted: exactly one minimal resend.
        script.insert(
            "https://push.test/toolarge".into(),
            vec![Ok(PushAnswer::status(413)), Ok(PushAnswer::status(201))],
        );
        // A timeout every time: two attempts (a timeout is retried once),
        // then counted.
        script.insert(
            "https://push.test/hang".into(),
            vec![Err(PushFailure::Timeout("operation timed out".into()))],
        );
        script.insert(
            "https://push.test/ok".into(),
            vec![Ok(PushAnswer::status(201))],
        );
        let captured = Arc::new(Mutex::new(Vec::new()));
        let post = scripted(script, captured.clone());
        let t0 = Instant::now();
        let mut big = note("big");
        big.title = "Ü".repeat(2500);
        deliver_push_with(&pg, &sb, &post, &user, &big, &quick(), "mailto:x@y").await;
        let posts = captured.lock().unwrap().clone();
        let of = |e: &str| {
            posts
                .iter()
                .filter(|(ep, _, _)| ep.ends_with(e))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(of("/throttled").len(), 2);
        let large = of("/toolarge");
        assert_eq!(large.len(), 2);
        assert!(large[1].2 < large[0].2, "the resend is the minimal payload");
        assert!(large[0].2 <= 4096, "even the first body fits the ceiling");
        assert_eq!(of("/hang").len(), 2);
        // The ok device did not wait behind the hang: it went first.
        assert!(of("/ok")[0].1.duration_since(t0) < Duration::from_millis(250));
        let hang = row_state(&pg, "https://push.test/hang").await.unwrap();
        assert_eq!(hang.0, 1);
        assert_eq!(hang.1, None);
        assert!(hang.2.unwrap().starts_with("timeout"));
        assert_eq!(
            row_state(&pg, "https://push.test/throttled")
                .await
                .unwrap()
                .1,
            Some(201)
        );
        assert_eq!(
            row_state(&pg, "https://push.test/toolarge")
                .await
                .unwrap()
                .1,
            Some(201)
        );
        cleanup(&pg).await;
    };
    with_vapid_row_set_aside(&pg, body).await;
}

/// A hung endpoint (accepts, never answers) costs at most two timed-out
/// tries, then sits out 2^failures minutes while the person's other device
/// keeps receiving; when its window closes it is tried again, and a 2xx
/// resets it. Three failures of any kind open the same window; ten with no
/// delivery for a week prune the row.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_hung_endpoint_sits_out_its_backoff_and_a_2xx_resets_it() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    let body = async {
        cleanup(&pg).await;
        let user = person(&pg, "backoff").await;
        let sb = test_box();
        for e in ["hang", "ok", "flaky", "dying"] {
            subscription(&pg, &user, &format!("https://push.test/{e}"), false).await;
        }
        let mut script = Script::new();
        script.insert(
            "https://push.test/hang".into(),
            vec![Err(PushFailure::Timeout("operation timed out".into()))],
        );
        for e in ["ok", "flaky", "dying"] {
            script.insert(
                format!("https://push.test/{e}"),
                vec![Ok(PushAnswer::status(201))],
            );
        }
        let captured = Arc::new(Mutex::new(Vec::new()));
        let post = scripted(script, captured.clone());
        let count = |e: &str| {
            captured
                .lock()
                .unwrap()
                .iter()
                .filter(|(ep, _, _)| ep.ends_with(e))
                .count()
        };
        deliver_push_with(&pg, &sb, &post, &user, &note("b1"), &quick(), "mailto:x@y").await;
        assert_eq!(count("/hang"), 2, "a timeout is tried twice at most");
        let hang = row_state(&pg, "https://push.test/hang").await.unwrap();
        assert_eq!((hang.0, hang.1), (1, None));
        // The next three deliveries skip it; the ok device gets each one.
        for i in 2..=4 {
            let t0 = Instant::now();
            deliver_push_with(
                &pg,
                &sb,
                &post,
                &user,
                &note(&format!("b{i}")),
                &quick(),
                "mailto:x@y",
            )
            .await;
            assert!(t0.elapsed() < Duration::from_secs(2));
        }
        assert_eq!(
            count("/hang"),
            2,
            "inside its window the hung row is skipped"
        );
        assert_eq!(count("/ok"), 4);
        // Its window (2^1 minutes) closes; it answers now, and is reset.
        sqlx::query(
            "update push_subscriptions set last_attempt_at = now() - interval '3 minutes' \
             where endpoint = 'https://push.test/hang'",
        )
        .execute(&pg)
        .await
        .unwrap();
        let mut healed = Script::new();
        for e in ["hang", "ok", "flaky", "dying"] {
            healed.insert(
                format!("https://push.test/{e}"),
                vec![Ok(PushAnswer::status(201))],
            );
        }
        let post2 = scripted(healed, captured.clone());
        deliver_push_with(&pg, &sb, &post2, &user, &note("b5"), &quick(), "mailto:x@y").await;
        assert_eq!(count("/hang"), 3);
        let hang = row_state(&pg, "https://push.test/hang").await.unwrap();
        assert_eq!((hang.0, hang.1), (0, Some(201)), "a 2xx resets the count");
        // Three failures of any kind: skipped for 2^3 minutes.
        sqlx::query(
            "update push_subscriptions set failures = 3, last_status = 500, \
               last_error = '500', last_attempt_at = now() - interval '7 minutes' \
             where endpoint = 'https://push.test/flaky'",
        )
        .execute(&pg)
        .await
        .unwrap();
        let f0 = count("/flaky");
        deliver_push_with(&pg, &sb, &post2, &user, &note("b6"), &quick(), "mailto:x@y").await;
        assert_eq!(
            count("/flaky"),
            f0,
            "seven minutes into an eight-minute window: skipped"
        );
        sqlx::query(
            "update push_subscriptions set last_attempt_at = now() - interval '9 minutes' \
             where endpoint = 'https://push.test/flaky'",
        )
        .execute(&pg)
        .await
        .unwrap();
        deliver_push_with(&pg, &sb, &post2, &user, &note("b7"), &quick(), "mailto:x@y").await;
        assert_eq!(
            count("/flaky"),
            f0 + 1,
            "past its window: tried, and reset by the 2xx"
        );
        assert_eq!(
            row_state(&pg, "https://push.test/flaky").await.unwrap().0,
            0
        );
        // Ten failures with no delivery for a week: pruned.
        sqlx::query(
            "update push_subscriptions set failures = 9, last_status = 500, last_error = '500', \
               last_attempt_at = now() - interval '2 hours', last_seen_at = now() - interval '8 days' \
             where endpoint = 'https://push.test/dying'",
        )
        .execute(&pg)
        .await
        .unwrap();
        let mut dying = Script::new();
        for e in ["hang", "ok", "flaky"] {
            dying.insert(
                format!("https://push.test/{e}"),
                vec![Ok(PushAnswer::status(201))],
            );
        }
        dying.insert(
            "https://push.test/dying".into(),
            vec![Ok(PushAnswer::status(500))],
        );
        let post3 = scripted(dying, captured.clone());
        deliver_push_with(&pg, &sb, &post3, &user, &note("b8"), &quick(), "mailto:x@y").await;
        assert!(!endpoint_exists(&pg, "https://push.test/dying").await);
        assert!(endpoint_exists(&pg, "https://push.test/ok").await);
        cleanup(&pg).await;
    };
    with_vapid_row_set_aside(&pg, body).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_vapid_keypair_is_born_once_even_under_a_race() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    with_vapid_row_set_aside(&pg, race_body(&pg)).await;
}

async fn race_body(pg: &PgPool) {
    let pg = pg.clone();
    let sb = test_box();

    // Four concurrent first calls — the boot overlap, two tabs, a deploy's
    // old and new process — race the INSERT; the on-conflict-do-nothing and
    // the re-read must converge every one of them on the winner's keypair.
    let mut racers = Vec::new();
    for _ in 0..4 {
        let pg = pg.clone();
        let sb = sb.clone();
        racers.push(tokio::spawn(async move { vapid_keys(&pg, &sb).await }));
    }
    let results: Vec<_> = futures_util::future::join_all(racers)
        .await
        .into_iter()
        .map(|r| r.expect("the racer task ran"))
        .collect();
    let first = results[0].as_ref().expect("the winner produced keys");
    for r in &results[1..] {
        let keys = r.as_ref().expect("every racer converged");
        assert_eq!(
            keys.public, first.public,
            "all racers hold the winner's key"
        );
    }

    // The public half is a real uncompressed point, and the row keeps the
    // private half SEALED — v2 grammar, not the DER it wraps.
    assert_eq!(first.public.len(), 65);
    assert_eq!(first.public[0], 0x04);
    let (private,): (String,) =
        sqlx::query_as("select value->>'private' from app_settings where key = $1")
            .bind(VAPID_KEY)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert!(
        private.starts_with("v2:"),
        "the private key is sealed: {private:?}"
    );
    let stored_public: String =
        sqlx::query_scalar("select value->>'public' from app_settings where key = $1")
            .bind(VAPID_KEY)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(stored_public, B64U.encode(first.public));

    // The seal is not theater: a box with a DIFFERENT key cannot read the
    // row back, so the DER never leaves the instance secret's reach.
    // (The unsealed keypair is cached by its sealed string — forget it, or
    // this read would be answered without opening the row at all.)
    let wrong = SecretBox::from_parts([7u8; 32], HashMap::from([(1u32, [11u8; 32])]), Some(1));
    talaria_api::push::forget_vapid_cache();
    assert!(vapid_keys(&pg, &wrong).await.is_err());

    // And a later read returns the SAME keypair — born once, read forever.
    let again = vapid_keys(&pg, &sb).await.expect("the second read works");
    assert_eq!(again.public, first.public);

    cleanup(&pg).await;
}

/// The single writer keeps only a same-origin path (talaria-notify
/// `openable_href`): a producer handed an absolute or protocol-relative href,
/// or a path with a dot segment that resolves to one, files the row with
/// `''`, so no click path — the bell, a toast, the worker, the shell — can be
/// walked off the instance by a notification.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_single_writer_stores_only_a_same_origin_href() {
    // The suite's lock: the other tests' cleanup deletes every push-live user.
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    sqlx::query("delete from users where email like 'push-live-href%@test.invalid'")
        .execute(&pg)
        .await
        .unwrap();
    let user = person(&pg, "href").await;
    let deps = NotifyDeps::publishing(pg.clone(), None);
    for (href, want) in [
        ("https://evil.example", ""),
        ("//evil.example", ""),
        ("/\\evil.example", ""),
        // Dot segments: `/..//evil.example` resolves to the PATH
        // `//evil.example`, another host once a client parses it again.
        ("/..//evil.example", ""),
        ("/.//evil.example", ""),
        ("/a/..//evil.example", ""),
        ("/%2e%2e//evil.example", ""),
        ("/boards/b/t", "/boards/b/t"),
    ] {
        add_notification(
            &deps,
            &user,
            &NotificationInput {
                kind: "mention",
                title: "href probe",
                body: Some(href),
                href: Some(href),
            },
        )
        .await
        .unwrap();
        let stored: String = sqlx::query_scalar(
            "select href from notifications where user_id = $1::uuid and body = $2 \
             order by created_at desc limit 1",
        )
        .bind(&user)
        .bind(href)
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(stored, want, "{href} stored as {stored:?}");
    }
    sqlx::query("delete from users where email like 'push-live-href%@test.invalid'")
        .execute(&pg)
        .await
        .unwrap();
}

/// One device browser's subscribe body with legal key halves.
fn subscribe_body(endpoint: &str) -> serde_json::Value {
    let secret = p256::SecretKey::generate();
    let point = secret.public_key().to_sec1_point(false);
    serde_json::json!({
        "endpoint": endpoint,
        "keys": { "p256dh": B64U.encode(point.as_bytes()), "auth": B64U.encode([3u8; 16]) },
    })
}

async fn endpoints_of(pg: &PgPool, user: &str) -> Vec<String> {
    sqlx::query_scalar(
        "select endpoint from push_subscriptions where user_id = $1::uuid order by endpoint",
    )
    .bind(user)
    .fetch_all(pg)
    .await
    .unwrap()
}

/// security-1 and security-3 at the door, through the REAL router: an
/// endpoint inside the server's network is refused before any row files —
/// one 400 for every rule, so the door is no oracle either — and a person
/// is held to twenty device browsers, the one seen longest ago retired
/// first, another person's untouched. The public endpoints are address
/// LITERALS so the test needs no DNS; nothing is ever posted to them.
#[tokio::test]
#[ignore = "needs a live dev database and redis (DATABASE_URL, REDIS_URL)"]
async fn the_subscribe_door_refuses_inside_endpoints_and_keeps_twenty_devices() {
    let _row = KEYPAIR_ROW.lock().await;
    let state = crate::support::live_app_state().await;
    let pg = state.pg.clone();
    cleanup(&pg).await;
    let (user, cookie) = crate::support::minted_member_with_email(
        &state,
        "push-live",
        "push-live-door@test.invalid",
    )
    .await;
    let (other, other_cookie) = crate::support::minted_member_with_email(
        &state,
        "push-live",
        "push-live-door-other@test.invalid",
    )
    .await;
    let subscribe = |cookie: String, endpoint: String| {
        let state = state.clone();
        async move {
            crate::support::call_json(
                &state,
                "POST",
                "/api/push/subscribe",
                Some(&cookie),
                Some(subscribe_body(&endpoint)),
            )
            .await
        }
    };

    for inside in [
        // Not 127.0.0.1 itself: the engine suite in this binary admits that
        // one address (TALARIA_FETCH_ALLOW_HOSTS) for its capture server.
        "https://127.0.0.53:5432/x",
        "https://localhost:5274/x",
        "https://[::1]/x",
        "https://169.254.169.254/latest/meta-data/",
        "https://10.255.255.1/x",
        "https://192.168.254.254/x",
        "https://100.64.0.1/x",
        "https://db.internal/x",
        "https://push.invalid/x",
    ] {
        let (status, body) = subscribe(cookie.clone(), inside.into()).await;
        assert_eq!(status.as_u16(), 400, "{inside} -> {body}");
        assert_eq!(
            body["error"], "Push endpoint refused: not a reachable public address",
            "{inside}"
        );
    }
    assert!(
        endpoints_of(&pg, &user).await.is_empty(),
        "no inside row filed"
    );

    // The other person's two devices, filed first, are not this person's to
    // retire.
    for i in 1..=2 {
        let (status, _) = subscribe(
            other_cookie.clone(),
            format!("https://1.1.1.1/push-live-other-{i:02}"),
        )
        .await;
        assert_eq!(status.as_u16(), 200);
    }
    let cap = |i: usize| format!("https://1.1.1.1/push-live-cap-{i:02}");
    for i in 1..=25 {
        let (status, body) = subscribe(cookie.clone(), cap(i)).await;
        assert_eq!(status.as_u16(), 200, "{i} -> {body}");
    }
    // Twenty kept: the five filed first are gone.
    assert_eq!(
        endpoints_of(&pg, &user).await,
        (6..=25).map(cap).collect::<Vec<_>>()
    );
    // Seen-longest-ago, not filed-first: re-filing 06 (a browser opening
    // Talaria again) makes 07 the one the next device retires.
    assert_eq!(subscribe(cookie.clone(), cap(6)).await.0.as_u16(), 200);
    assert_eq!(subscribe(cookie.clone(), cap(26)).await.0.as_u16(), 200);
    let kept = endpoints_of(&pg, &user).await;
    assert_eq!(kept.len(), 20);
    assert!(kept.contains(&cap(6)) && kept.contains(&cap(26)));
    assert!(!kept.contains(&cap(7)));
    assert_eq!(endpoints_of(&pg, &other).await.len(), 2);

    cleanup(&pg).await;
}

/// security-3 at delivery: a person filed past the cap before it existed
/// still gets one notification as at most twenty POSTs — to the twenty seen
/// most recently.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_delivery_posts_to_at_most_twenty_devices() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pg().await;
    with_vapid_row_set_aside(&pg, fan_body(&pg)).await;
}

async fn fan_body(pg: &PgPool) {
    let pg = pg.clone();
    cleanup(&pg).await;
    let user = person(&pg, "fan").await;
    for i in 0..25 {
        let secret = p256::SecretKey::generate();
        let point = secret.public_key().to_sec1_point(false);
        sqlx::query(
            "insert into push_subscriptions (user_id, endpoint, p256dh, auth, last_seen_at) \
             values ($1::uuid, $2, $3, $4, now() - make_interval(mins => $5))",
        )
        .bind(&user)
        .bind(format!("https://push.test/fan-{i:02}"))
        .bind(B64U.encode(point.as_bytes()))
        .bind(B64U.encode([3u8; 16]))
        .bind(i)
        .execute(&pg)
        .await
        .unwrap();
    }
    let captured: Arc<Mutex<Vec<PushPost>>> = Arc::new(Mutex::new(Vec::new()));
    deliver_push_with(
        &pg,
        &test_box(),
        &fake_post(captured.clone()),
        &user,
        &note("note-fan"),
        &quick(),
        "https://talaria.example.test",
    )
    .await;
    let mut posted: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.endpoint.clone())
        .collect();
    posted.sort_unstable();
    assert_eq!(
        posted,
        (0..20)
            .map(|i| format!("https://push.test/fan-{i:02}"))
            .collect::<Vec<_>>()
    );
    cleanup(&pg).await;
}

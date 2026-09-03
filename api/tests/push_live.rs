// Live-DB proof of the push plane's row tending and keypair custody
// (cargo test -- --ignored). The cryptography is pinned to RFC vectors in
// the unit tests; what only Postgres can confirm is the LOOP — which rows
// a delivery keeps, touches, or prunes, and that the instance's VAPID
// keypair is born exactly once even when several first deliveries race.
// House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test push_live -- --ignored

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use sqlx::postgres::PgPool;
use talaria_api::push::{PostPushFn, PushNote, PushPost, VAPID_KEY, deliver_push, vapid_keys};
use talaria_api::secretbox::SecretBox;

const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The suite's instance-row lock. Both tests BIRTH and DELETE the one
/// app_settings keypair row (the race test has no other subject), and cargo
/// runs them concurrently by default — one test's cleanup landing between
/// the other's insert and its re-read is a spurious "the vapid keypair row
/// vanished", a flake about scheduling, not about the plane.
static KEYPAIR_ROW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// The suite's throwaway person and its keypair row: the user cascade takes
/// push_subscriptions with it, and the instance keypair row is deleted so
/// every test births its own.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'push-live-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from app_settings where key = $1")
        .bind(VAPID_KEY)
        .execute(pg)
        .await
        .unwrap();
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
            Ok(code)
        })
    })
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn delivery_touches_prunes_and_keeps_by_the_services_answer() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pool().await;
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
    deliver_push(
        &pg,
        &sb,
        &post,
        &user,
        &PushNote {
            id: "note-1".into(),
            title: "Priya says".into(),
            body: "while you were away".into(),
            href: "/comms/agent/claude-test/c1".into(),
        },
    )
    .await;

    // Every LEGAL row was posted exactly once; the garbage row never was.
    let posts = captured.lock().unwrap();
    let mut posted: Vec<&str> = posts.iter().map(|p| p.endpoint.as_str()).collect();
    posted.sort_unstable();
    assert_eq!(
        posted,
        [
            "https://push.test/a",
            "https://push.test/b",
            "https://push.test/c",
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
    }

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

    cleanup(&pg).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_vapid_keypair_is_born_once_even_under_a_race() {
    let _row = KEYPAIR_ROW.lock().await;
    let pg = pool().await;
    cleanup(&pg).await;
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
    let wrong = SecretBox::from_parts([7u8; 32], HashMap::from([(1u32, [11u8; 32])]), Some(1));
    assert!(vapid_keys(&pg, &wrong).await.is_err());

    // And a later read returns the SAME keypair — born once, read forever.
    let again = vapid_keys(&pg, &sb).await.expect("the second read works");
    assert_eq!(again.public, first.public);

    cleanup(&pg).await;
}

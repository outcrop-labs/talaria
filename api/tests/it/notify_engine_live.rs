// Live proof of the delivery engine (docs/notifications/delivery.md) against a
// real Postgres and Redis, with a real push transport aimed at an in-process
// capture server: the decision at write time (the matrix, DND, the
// break-through, quiet Following), the presence-deferred push and its ack,
// the push headers by category, the missed-activity batcher (with a fake mail
// edge), retention, the push prune of ended sessions, the per-device test and
// the VAPID key cache, and the workspace zone a no-zone person reads.
//
// ONE test, steps in order: the push plane and the presence handle are
// process-wide OnceLocks, so they are installed once, in this test's runtime,
// and nothing else in the module may race them. The instance's own
// `push_vapid`, `notify_delivery` and `brief_config` rows are set aside and
// restored, pass or fail, and the people are throwaway `.invalid` rows. Stop
// the stack's own api while it runs (its batcher would claim these rows too).
//
//   DATABASE_URL=… REDIS_URL=… cargo test -p talaria-api --test it \
//     notify_engine_live:: -- --ignored --nocapture

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::FutureExt as _;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use talaria_api::notify::presence::{self, ClientKind, ClientState, Heartbeat};
use talaria_api::notify::{
    Coalesce, GatedSendResult, MissedMail, MissedMailDeps, MissedSendFn, NotificationInput,
    NotifyDeps, NotifyMeta, add_notification_with, defer, group_key, retention,
};
use talaria_api::push::{VAPID_KEY, hkdf_expand, hkdf_extract};
use talaria_api::realtime::RealtimeDeps;
use talaria_api::secretbox::SecretBox;

const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

// ── the capture server: a push service on 127.0.0.1 ────────────────────────

#[derive(Clone, Debug)]
struct Hit {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Clone)]
struct Capture {
    base: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Capture {
    fn on(&self, path: &str) -> Vec<Hit> {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.path == path)
            .cloned()
            .collect()
    }
    async fn wait(&self, path: &str, n: usize, within: Duration) -> Vec<Hit> {
        let end = Instant::now() + within;
        loop {
            let got = self.on(path);
            if got.len() >= n || Instant::now() >= end {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// HTTP/1.1, one request per connection, answered by the path's last
/// segment: `…-hang` never answers, `…-err` is a 500, anything else 201.
async fn capture() -> Capture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits: Arc<Mutex<Vec<Hit>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let head_end = loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.split("\r\n");
                let path = lines
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_string();
                let headers: HashMap<String, String> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                let len: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = buf[head_end + 4..].to_vec();
                while body.len() < len {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                sink.lock().unwrap().push(Hit {
                    path: path.clone(),
                    headers,
                    body,
                });
                if path.ends_with("-hang") {
                    tokio::time::sleep(Duration::from_secs(40)).await;
                    return;
                }
                let status = if path.ends_with("-err") {
                    "500 Internal Server Error"
                } else {
                    "201 Created"
                };
                let _ = sock
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    Capture { base, hits }
}

// ── the "browser": a subscription's key material, and the decrypt ───────────

struct Browser {
    secret: p256::SecretKey,
    public: [u8; 65],
    auth: [u8; 16],
}

impl Browser {
    fn new() -> Self {
        let secret = p256::SecretKey::generate();
        let public: [u8; 65] = secret
            .public_key()
            .to_sec1_point(false)
            .as_bytes()
            .try_into()
            .unwrap();
        Self {
            secret,
            public,
            auth: [5u8; 16],
        }
    }

    /// RFC 8291: the payload the service worker would see.
    fn decrypt(&self, body: &[u8]) -> Value {
        use aes_gcm::aead::{Aead, KeyInit};
        let salt = &body[..16];
        let idlen = body[20] as usize;
        let keyid = &body[21..21 + idlen];
        let ct = &body[21 + idlen..];
        let eph = p256::PublicKey::from_sec1_bytes(keyid).unwrap();
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), eph.as_affine());
        let prk_key = hkdf_extract(&self.auth, shared.raw_secret_bytes());
        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(&self.public);
        info.extend_from_slice(keyid);
        let ikm = hkdf_expand(&prk_key, &info, 32);
        let prk = hkdf_extract(salt, &ikm);
        let cek = hkdf_expand(&prk, b"Content-Encoding: aes128gcm\0", 16);
        let nonce: [u8; 12] = hkdf_expand(&prk, b"Content-Encoding: nonce\0", 12)
            .try_into()
            .unwrap();
        let cipher = aes_gcm::Aes128Gcm::new_from_slice(&cek).unwrap();
        let mut plain = cipher
            .decrypt(&aes_gcm::Nonce::from(nonce), ct)
            .expect("the payload decrypts with the subscription's keys");
        while plain.last() == Some(&0) {
            plain.pop();
        }
        assert_eq!(plain.pop(), Some(2), "one record, delimited");
        serde_json::from_slice(&plain).unwrap()
    }
}

// ── people, rows, the instance rows set aside ───────────────────────────────

const TAG: &str = "notify-engine";

async fn wide_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect(crate::support::DATABASE_URL_EXPECT);
    PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(60))
        .connect(&url)
        .await
        .unwrap()
}

async fn redis_conn() -> redis::aio::ConnectionManager {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6399".into());
    let client = redis::Client::open(url).unwrap();
    redis::aio::ConnectionManager::new(client).await.unwrap()
}

async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'notify-engine-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, name: &str) -> String {
    sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') returning id::text",
    )
    .bind(format!("{TAG}-{name}"))
    .bind(format!("{TAG}-{name}@test.invalid"))
    .bind(format!("Engine {name}"))
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn set_prefs(pg: &PgPool, user: &str, prefs: Value) {
    sqlx::query("update users set notify_prefs = $2, timezone = null where id = $1::uuid")
        .bind(user)
        .bind(prefs)
        .execute(pg)
        .await
        .unwrap();
}

/// One push subscription for `user` at `<capture>/<path>`; answers its id.
async fn subscribe(pg: &PgPool, user: &str, browser: &Browser, endpoint: &str) -> String {
    sqlx::query_scalar(
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth) \
         values ($1::uuid, $2, $3, $4) returning id::text",
    )
    .bind(user)
    .bind(endpoint)
    .bind(B64U.encode(browser.public))
    .bind(B64U.encode(browser.auth))
    .fetch_one(pg)
    .await
    .unwrap()
}

fn deps(pg: &PgPool) -> NotifyDeps {
    NotifyDeps {
        pg: pg.clone(),
        realtime: RealtimeDeps {
            publish: Arc::new(|_: &str, _: &str| {}),
            subscribe: Arc::new(|_| async { tokio::sync::mpsc::channel::<String>(1).1 }.boxed()),
        },
    }
}

/// The delivery columns of one row, as epoch ms (null → None).
#[derive(Debug, Clone, PartialEq)]
struct Row {
    alert: Option<String>,
    pushed: Option<i64>,
    push_due: Option<i64>,
    email_due: Option<i64>,
    emailed: Option<i64>,
    attempts: i32,
    read: bool,
    now: i64,
}

async fn row(pg: &PgPool, id: &str) -> Row {
    let ms = |c: &str| format!("(trunc(extract(epoch from {c}) * 1000))::bigint");
    let (alert, pushed, push_due, email_due, emailed, attempts, read, now): (
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        i32,
        bool,
        i64,
    ) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select alert, {}, {}, {}, {}, email_attempts, read_at is not null, {} \
         from notifications where id = $1::uuid",
        ms("pushed_at"),
        ms("push_due_at"),
        ms("email_due_at"),
        ms("emailed_at"),
        ms("now()")
    )))
    .bind(id)
    .fetch_one(pg)
    .await
    .unwrap();
    Row {
        alert,
        pushed,
        push_due,
        email_due,
        emailed,
        attempts,
        read,
        now,
    }
}

async fn file(pg: &PgPool, user: &str, kind: &str, title: &str, meta: &NotifyMeta) -> String {
    add_notification_with(
        &deps(pg),
        user,
        &NotificationInput {
            kind,
            title,
            body: Some("from the engine suite"),
            href: Some("/boards/b/t"),
        },
        meta,
    )
    .await
    .unwrap()
    .id
}

async fn exec(pg: &PgPool, sql: &str, id: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
        .bind(id)
        .execute(pg)
        .await
        .unwrap();
}

/// Run `body` with the instance's `push_vapid`, `notify_delivery` and
/// `brief_config` rows set aside, then put them back — pass or fail.
async fn with_instance_rows_set_aside<F: std::future::Future<Output = ()>>(pg: &PgPool, body: F) {
    let keys = [VAPID_KEY, "notify_delivery", "brief_config"];
    let mut saved: Vec<(String, Value)> = Vec::new();
    for k in keys {
        if let Some(v) =
            sqlx::query_scalar::<_, Value>("select value from app_settings where key = $1")
                .bind(k)
                .fetch_optional(pg)
                .await
                .unwrap()
        {
            saved.push((k.to_string(), v));
        }
        sqlx::query("delete from app_settings where key = $1")
            .bind(k)
            .execute(pg)
            .await
            .unwrap();
    }
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    for k in keys {
        sqlx::query("delete from app_settings where key = $1")
            .bind(k)
            .execute(pg)
            .await
            .unwrap();
    }
    for (k, v) in saved {
        sqlx::query("insert into app_settings (key, value) values ($1, $2)")
            .bind(k)
            .bind(v)
            .execute(pg)
            .await
            .unwrap();
    }
    cleanup(pg).await;
    if let Err(p) = outcome {
        std::panic::resume_unwind(p);
    }
}

fn test_box() -> SecretBox {
    SecretBox::from_parts([7u8; 32], HashMap::from([(1u32, [9u8; 32])]), Some(1))
}

/// A fake mail edge: records every mail and answers from a script (the last
/// answer repeats; empty = accepted).
#[derive(Clone, Default)]
struct FakeMail {
    sent: Arc<Mutex<Vec<MissedMail>>>,
    answers: Arc<Mutex<Vec<GatedSendResult>>>,
}

impl FakeMail {
    fn deps(&self, pg: &PgPool) -> MissedMailDeps {
        let me = self.clone();
        let send: MissedSendFn = Arc::new(move |m: MissedMail| {
            let me = me.clone();
            Box::pin(async move {
                me.sent.lock().unwrap().push(m);
                let mut a = me.answers.lock().unwrap();
                match a.len() {
                    0 => GatedSendResult {
                        ok: true,
                        blocked: false,
                        error: None,
                    },
                    1 => a[0].clone(),
                    _ => a.remove(0),
                }
            })
        });
        MissedMailDeps {
            pg: pg.clone(),
            realtime: deps(pg).realtime,
            send,
        }
    }
    fn answer(&self, r: GatedSendResult) {
        *self.answers.lock().unwrap() = vec![r];
    }
    fn mails_to(&self, user: &str) -> Vec<MissedMail> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.user_id == user)
            .cloned()
            .collect()
    }
}

async fn due_now(pg: &PgPool, ids: &[String]) {
    sqlx::query(
        "update notifications set email_due_at = now() - interval '1 second' where id = any($1::uuid[])",
    )
    .bind(ids)
    .execute(pg)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (DATABASE_URL, REDIS_URL)"]
async fn the_delivery_engine() {
    let pg = wide_pool().await;
    cleanup(&pg).await;
    with_instance_rows_set_aside(&pg, engine(&pg)).await;
}

async fn engine(pg: &PgPool) {
    let pg = pg.clone();
    sqlx::query(
        "insert into app_settings (key, value) values ('notify_delivery', '{\"emailEnabled\": true}')",
    )
    .execute(&pg)
    .await
    .unwrap();
    talaria_api::push::install_push_plane(pg.clone(), test_box());
    let mut conn = redis_conn().await;
    presence::install_presence_redis(conn.clone());
    let cap = capture().await;
    let browser = Browser::new();
    let ada = person(&pg, "ada").await;
    let bob = person(&pg, "bob").await;
    let ada_dev = subscribe(&pg, &ada, &browser, &format!("{}/ada-ok", cap.base)).await;
    let bob_dev = subscribe(&pg, &bob, &browser, &format!("{}/bob-ok", cap.base)).await;
    let m = NotifyMeta::default();
    let pres = |u: &str| format!("presence:dev:{u}");
    let _: () = redis::cmd("DEL")
        .arg(pres(&ada))
        .arg(format!("presence:win:{ada}"))
        .query_async(&mut conn)
        .await
        .unwrap();

    // ── 1. The matrix ──────────────────────────────────────────────────────
    set_prefs(
        &pg,
        &ada,
        json!({ "channels": { "mention": { "desktop": false } } }),
    )
    .await;
    let id = file(&pg, &ada, "mention", "desktop off", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!(r.alert.as_deref(), Some("feed"), "Desktop off → a feed row");
    assert!(r.pushed.is_some(), "Push is its own channel");
    assert!(
        (r.email_due.unwrap() - r.now - 3_600_000).abs() < 60_000,
        "Email on → mail due in an hour: {r:?}"
    );
    let pushed = cap.wait("/ada-ok", 1, Duration::from_secs(5)).await;
    assert_eq!(pushed.len(), 1);
    set_prefs(
        &pg,
        &ada,
        json!({ "channels": { "mention": { "push": false, "email": false } } }),
    )
    .await;
    let id = file(&pg, &ada, "mention", "push off", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!(
        (r.alert.as_deref(), r.pushed, r.push_due, r.email_due),
        (Some("banner"), None, None, None)
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        cap.on("/ada-ok").len(),
        1,
        "Push off → nothing reaches the device"
    );
    println!("PASS matrix");

    // ── 2. DND, working hours, the break-through ──────────────────────────
    let until = chrono::Utc::now() + chrono::Duration::hours(1);
    let until_iso = until.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    set_prefs(&pg, &ada, json!({ "dnd": { "pausedUntil": until_iso } })).await;
    let id = file(&pg, &ada, "dm", "a DM during a pause", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!(
        (r.alert.as_deref(), r.pushed, r.push_due),
        (Some("feed"), None, None)
    );
    assert!(
        r.email_due.unwrap() >= until.timestamp_millis() - 1,
        "mail waits for the pause: {r:?}"
    );
    let before = cap.on("/ada-ok").len();
    let id = file(
        &pg,
        &ada,
        "approval_pending",
        "an approval during a pause",
        &m,
    )
    .await;
    let r = row(&pg, &id).await;
    assert_eq!(
        r.alert.as_deref(),
        Some("banner"),
        "Needs you breaks through"
    );
    assert!(r.pushed.is_some());
    let hits = cap
        .wait("/ada-ok", before + 1, Duration::from_secs(5))
        .await;
    let h = hits.last().unwrap();
    assert_eq!(h.headers.get("urgency").map(String::as_str), Some("high"));
    assert_eq!(h.headers.get("ttl").map(String::as_str), Some("86400"));
    assert_eq!(h.headers.get("topic"), Some(&id.replace('-', "")));
    let p = browser.decrypt(&h.body);
    assert_eq!(
        (p["id"].as_str(), p["kind"].as_str()),
        (Some(id.as_str()), Some("approval_pending"))
    );
    set_prefs(
        &pg,
        &ada,
        json!({ "dnd": { "pausedUntil": until_iso, "allowNeedsYou": false } }),
    )
    .await;
    let id = file(&pg, &ada, "approval_pending", "no break-through", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!((r.alert.as_deref(), r.pushed), (Some("feed"), None));
    // Working hours that exclude now (an hour, three hours from now) are a pause.
    let start = (chrono::Utc::now() + chrono::Duration::hours(3))
        .format("%H:00")
        .to_string();
    let end = (chrono::Utc::now() + chrono::Duration::hours(4))
        .format("%H:00")
        .to_string();
    set_prefs(
        &pg,
        &ada,
        json!({ "dnd": { "schedule": { "enabled": true, "days": [0,1,2,3,4,5,6], "start": start, "end": end } } }),
    )
    .await;
    let id = file(&pg, &ada, "dm", "outside working hours", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!(
        (r.alert.as_deref(), r.pushed, r.push_due),
        (Some("feed"), None, None)
    );
    assert!(
        r.email_due.unwrap() > r.now + 2 * 3_600_000,
        "mail when the hours start: {r:?}"
    );
    set_prefs(&pg, &ada, json!({})).await;
    println!("PASS dnd");

    // ── 3. Defaults: Following quiet, a stored route kept, account ─────────
    let before = cap.on("/ada-ok").len();
    let id = file(&pg, &ada, "task-status", "a ticket moved", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!(
        (r.alert.as_deref(), r.pushed, r.email_due),
        (Some("feed"), None, None)
    );
    set_prefs(&pg, &ada, json!({ "work_complete": "both" })).await;
    let id = file(&pg, &ada, "task-status", "a ticket moved, route both", &m).await;
    let r = row(&pg, &id).await;
    assert_eq!(r.alert.as_deref(), Some("banner"));
    assert!(r.pushed.is_some() && r.email_due.is_some(), "{r:?}");
    set_prefs(&pg, &ada, json!({})).await;
    let id = file(
        &pg,
        &ada,
        "agent_google_principal",
        "an agent acts as you",
        &m,
    )
    .await;
    let r = row(&pg, &id).await;
    assert_eq!(r.alert.as_deref(), Some("banner"));
    assert!(
        r.pushed.is_some() && r.email_due.is_some(),
        "account alerts and mails: {r:?}"
    );
    let hits = cap
        .wait("/ada-ok", before + 2, Duration::from_secs(5))
        .await;
    assert_eq!(
        hits.len(),
        before + 2,
        "both the route-both row and the account row pushed"
    );
    println!("PASS defaults");

    // ── 4. Presence: the deferred push, the ack, the read ──────────────────
    let beat = |state| Heartbeat {
        client_id: "tab-engine-01".into(),
        kind: ClientKind::Web,
        state,
        os_idle_ms: None,
    };
    let now = || talaria_api::agent_auth::now_ms();
    presence::record(
        &deps(&pg),
        &mut conn,
        &ada,
        &beat(ClientState::Looking),
        now(),
    )
    .await
    .unwrap();
    let before = cap.on("/ada-ok").len();
    let looked = file(&pg, &ada, "dm", "while looking", &m).await;
    let r = row(&pg, &looked).await;
    assert_eq!(r.pushed, None, "a looking client defers the push");
    assert!(
        (r.push_due.unwrap() - r.now - 60_000).abs() < 2_000,
        "{r:?}"
    );
    let acked = file(&pg, &ada, "dm", "acked", &m).await;
    let read = file(&pg, &ada, "dm", "read", &m).await;
    assert_eq!(
        defer::ack(&pg, &ada, std::slice::from_ref(&acked))
            .await
            .unwrap(),
        1
    );
    exec(
        &pg,
        "update notifications set read_at = now() where id = $1::uuid",
        &read,
    )
    .await;
    // Nothing yet: not due.
    defer::run_deferred_push(&pg).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        cap.on("/ada-ok").len(),
        before,
        "nothing pushes before the deferral is due"
    );
    for id in [&looked, &acked, &read] {
        exec(&pg, "update notifications set push_due_at = now() - interval '1 second' where id = $1::uuid", id).await;
    }
    let msg = defer::run_deferred_push(&pg).await.unwrap();
    let hits = cap
        .wait("/ada-ok", before + 1, Duration::from_secs(5))
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        cap.on("/ada-ok").len(),
        before + 1,
        "only the un-acked, unread row pushed: {msg:?}"
    );
    assert_eq!(browser.decrypt(&hits.last().unwrap().body)["id"], looked);
    assert!(row(&pg, &looked).await.pushed.is_some());
    assert_eq!(
        row(&pg, &acked).await.pushed,
        None,
        "the ack cancels the push"
    );
    assert_eq!(
        row(&pg, &read).await.pushed,
        None,
        "the read cancels the push"
    );
    // A second pass pushes nothing again (claimed once).
    defer::run_deferred_push(&pg).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(cap.on("/ada-ok").len(), before + 1);
    // An active desktop defers the same way.
    presence::record(
        &deps(&pg),
        &mut conn,
        &ada,
        &Heartbeat {
            client_id: "desk-engine-01".into(),
            kind: ClientKind::Desktop,
            state: ClientState::Active,
            os_idle_ms: Some(1_000),
        },
        now(),
    )
    .await
    .unwrap();
    presence::record(
        &deps(&pg),
        &mut conn,
        &ada,
        &beat(ClientState::Hidden),
        now(),
    )
    .await
    .unwrap();
    let id = file(&pg, &ada, "dm", "desktop active", &m).await;
    assert!(
        row(&pg, &id).await.push_due.is_some(),
        "an active desktop defers"
    );
    // Nobody there: a push now.
    let _: () = redis::cmd("DEL")
        .arg(pres(&ada))
        .query_async(&mut conn)
        .await
        .unwrap();
    let before = cap.on("/ada-ok").len();
    let t0 = Instant::now();
    let id = file(&pg, &ada, "dm", "nobody there", &m).await;
    assert!(row(&pg, &id).await.pushed.is_some());
    cap.wait("/ada-ok", before + 1, Duration::from_secs(2))
        .await;
    assert_eq!(
        cap.on("/ada-ok").len(),
        before + 1,
        "no heartbeat → a push within 2 s"
    );
    assert!(t0.elapsed() < Duration::from_secs(2));
    println!("PASS presence");

    // ── 5. A re-alert reuses the Topic ─────────────────────────────────────
    let before = cap.on("/ada-ok").len();
    let keyed = NotifyMeta {
        group_key: Some(group_key::dm("engine-dm")),
        coalesce: Coalesce::Realert,
        ..Default::default()
    };
    let first = file(&pg, &ada, "dm", "one", &keyed).await;
    let second = file(&pg, &ada, "dm", "two", &keyed).await;
    assert_eq!(first, second, "one open row per group");
    let hits = cap
        .wait("/ada-ok", before + 2, Duration::from_secs(5))
        .await;
    // The two pushes are detached tasks, so they may reach the service in
    // either order: tell them apart by what they carry, not by arrival.
    let pair = &hits[hits.len() - 2..];
    assert_eq!(pair[0].headers.get("topic"), pair[1].headers.get("topic"));
    assert_eq!(pair[0].headers.get("topic"), Some(&first.replace('-', "")));
    let realert = pair
        .iter()
        .find(|h| browser.decrypt(&h.body)["renotify"] == true)
        .expect("one of the two is the re-alert");
    assert_eq!(browser.decrypt(&realert.body)["title"], "two");
    assert_eq!(
        realert.headers.get("urgency").map(String::as_str),
        Some("normal")
    );
    assert_eq!(
        realert.headers.get("ttl").map(String::as_str),
        Some("43200")
    );
    println!("PASS topic");

    // ── 6. The missed-activity batcher ─────────────────────────────────────
    let mail = FakeMail::default();
    let md = mail.deps(&pg);
    set_prefs(&pg, &bob, json!({ "email": { "frequency": "15m" } })).await;
    let mut four = Vec::new();
    for i in 0..3 {
        four.push(file(&pg, &bob, "dm", &format!("dm {i}"), &m).await);
    }
    four.push(file(&pg, &bob, "mention", "a mention", &m).await);
    let r = row(&pg, &four[0]).await;
    assert!(
        (r.email_due.unwrap() - r.now - 15 * 60_000).abs() < 60_000,
        "15m: {r:?}"
    );
    // Not due yet: nothing.
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    assert!(mail.mails_to(&bob).is_empty());
    due_now(&pg, &four).await;
    let pass = talaria_api::notify::run_missed_mail(&md).await.unwrap();
    let mails = mail.mails_to(&bob);
    assert_eq!(mails.len(), 1, "one mail: {pass:?}");
    assert_eq!(mails[0].ids.len(), 4);
    assert_eq!(mails[0].subject, "4 unread in Talaria");
    assert!(mails[0].text.contains("DIRECT") && mails[0].text.contains("a mention"));
    for id in &four {
        assert!(row(&pg, id).await.emailed.is_some());
    }
    // A conversation already mailed and still unread is armed again by its
    // next message — a refresh in place, or a Realert coalesce — at the
    // person's frequency; the next batch carries its newest content. A mail
    // already pending stands.
    let refreshed = talaria_api::notify::refresh_notification(
        &deps(&pg),
        &four[0],
        &bob,
        &NotificationInput {
            kind: "dm",
            title: "dm 0, a newer message",
            body: Some("the newest line"),
            href: Some("/boards/b/t"),
        },
    )
    .await
    .unwrap();
    assert!(refreshed);
    let r = row(&pg, &four[0]).await;
    assert_eq!((r.emailed, r.attempts), (None, 0), "re-armed: {r:?}");
    assert!(
        (r.email_due.unwrap() - r.now - 15 * 60_000).abs() < 60_000,
        "at the frequency: {r:?}"
    );
    let grouped = NotifyMeta {
        group_key: Some(group_key::dm("engine-mail-regroup")),
        coalesce: Coalesce::Realert,
        ..Default::default()
    };
    let g1 = file(&pg, &bob, "dm", "grouped one", &grouped).await;
    due_now(&pg, std::slice::from_ref(&g1)).await;
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    let after_first = mail.mails_to(&bob);
    assert_eq!(
        after_first.len(),
        2,
        "the grouped row and the refreshed one"
    );
    assert!(after_first[1].ids.contains(&g1) && after_first[1].ids.contains(&four[0]));
    assert!(after_first[1].text.contains("dm 0, a newer message"));
    let g2 = file(&pg, &bob, "dm", "grouped two", &grouped).await;
    assert_eq!(g1, g2);
    let r = row(&pg, &g1).await;
    assert_eq!(r.emailed, None, "a Realert re-arms a mailed row: {r:?}");
    let pending = r.email_due;
    let g3 = file(&pg, &bob, "dm", "grouped three", &grouped).await;
    assert_eq!(
        row(&pg, &g3).await.email_due,
        pending,
        "a pending mail stands"
    );
    due_now(&pg, std::slice::from_ref(&g1)).await;
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    let mails = mail.mails_to(&bob);
    assert_eq!(mails.len(), 3);
    assert_eq!(mails[2].ids, std::slice::from_ref(&g1));
    assert!(mails[2].text.contains("grouped three"));
    sqlx::query("update notifications set read_at = now() where id = any($1::uuid[])")
        .bind([g1.clone(), four[0].clone()])
        .execute(&pg)
        .await
        .unwrap();
    // One read before due → three.
    let mut more = Vec::new();
    for i in 0..4 {
        more.push(file(&pg, &bob, "dm", &format!("again {i}"), &m).await);
    }
    exec(
        &pg,
        "update notifications set read_at = now() where id = $1::uuid",
        &more[0],
    )
    .await;
    due_now(&pg, &more).await;
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    let mails = mail.mails_to(&bob);
    assert_eq!(mails.len(), 4);
    assert_eq!(mails[3].ids.len(), 3, "a row read first is never mailed");
    assert_eq!(row(&pg, &more[0]).await.emailed, None);
    // All read → no mail.
    let mut read_all = Vec::new();
    for i in 0..2 {
        read_all.push(file(&pg, &bob, "dm", &format!("read {i}"), &m).await);
    }
    sqlx::query("update notifications set read_at = now() where id = any($1::uuid[])")
        .bind(&read_all)
        .execute(&pg)
        .await
        .unwrap();
    due_now(&pg, &read_all).await;
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    assert_eq!(mail.mails_to(&bob).len(), 4, "every row read → no mail");
    // A transport failure: attempts 1, 2, then given up.
    mail.answer(GatedSendResult {
        ok: false,
        blocked: false,
        error: Some("550 mailbox unavailable".into()),
    });
    let failing = file(&pg, &bob, "dm", "a failing mail", &m).await;
    for attempt in 1..=3 {
        due_now(&pg, std::slice::from_ref(&failing)).await;
        let pass = talaria_api::notify::run_missed_mail(&md).await.unwrap();
        let r = row(&pg, &failing).await;
        assert_eq!(r.attempts, attempt, "{pass:?}");
        if attempt < 3 {
            let wait = if attempt == 1 { 5 } else { 15 } * 60_000;
            assert!(
                (r.email_due.unwrap() - r.now - wait).abs() < 60_000,
                "backoff: {r:?}"
            );
        } else {
            assert_eq!((r.email_due, r.emailed), (None, None), "given up: {r:?}");
            assert_eq!(pass.gave_up, 1);
        }
    }
    assert_eq!(mail.mails_to(&bob).len(), 7);
    // The switch refused mid-pass (blocked) → discarded, nothing retried.
    mail.answer(GatedSendResult {
        ok: false,
        blocked: true,
        error: Some("off".into()),
    });
    let blocked = file(&pg, &bob, "dm", "blocked", &m).await;
    due_now(&pg, std::slice::from_ref(&blocked)).await;
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    let r = row(&pg, &blocked).await;
    assert_eq!((r.email_due, r.emailed, r.attempts), (None, None, 0));
    // The kill switch off → due rows dropped, nothing offered at all.
    mail.answer(GatedSendResult {
        ok: true,
        blocked: false,
        error: None,
    });
    let off = file(&pg, &bob, "dm", "mail switched off", &m).await;
    sqlx::query(
        "update app_settings set value = '{\"emailEnabled\": false}' where key = 'notify_delivery'",
    )
    .execute(&pg)
    .await
    .unwrap();
    due_now(&pg, std::slice::from_ref(&off)).await;
    let sent_before = mail.sent.lock().unwrap().len();
    let pass = talaria_api::notify::run_missed_mail(&md).await.unwrap();
    assert_eq!(
        mail.sent.lock().unwrap().len(),
        sent_before,
        "kill switch off → nothing sent"
    );
    assert!(pass.discarded >= 1);
    assert_eq!(row(&pg, &off).await.email_due, None);
    sqlx::query(
        "update app_settings set value = '{\"emailEnabled\": true}' where key = 'notify_delivery'",
    )
    .execute(&pg)
    .await
    .unwrap();
    // The legacy email-only route: read once a provider took the mail.
    set_prefs(&pg, &bob, json!({ "dm": "email" })).await;
    let only = file(&pg, &bob, "dm", "email only", &m).await;
    let r = row(&pg, &only).await;
    assert_eq!(
        (r.alert.as_deref(), r.pushed, r.read),
        (Some("feed"), None, false)
    );
    due_now(&pg, std::slice::from_ref(&only)).await;
    talaria_api::notify::run_missed_mail(&md).await.unwrap();
    let r = row(&pg, &only).await;
    assert!(
        r.emailed.is_some() && r.read,
        "email-only: read after the accepted mail: {r:?}"
    );
    set_prefs(&pg, &bob, json!({})).await;
    println!("PASS missed-activity mail");

    // ── 7. Retention ───────────────────────────────────────────────────────
    let aged: Vec<String> = sqlx::query_scalar(
        "insert into notifications (user_id, kind, title, read_at, created_at) values \
           ($1::uuid, 'dm', 'read 91 d', now() - interval '91 days', now() - interval '92 days'), \
           ($1::uuid, 'dm', 'unread 181 d', null, now() - interval '181 days'), \
           ($1::uuid, 'dm', 'read 30 d', now() - interval '30 days', now() - interval '31 days'), \
           ($1::uuid, 'dm', 'unread 100 d', null, now() - interval '100 days') \
         returning id::text",
    )
    .bind(&bob)
    .fetch_all(&pg)
    .await
    .unwrap();
    // An unread group row born 181 days ago that a Quiet coalesce updated
    // yesterday: its age is its last activity, so it stays.
    let quiet_group: String = sqlx::query_scalar(
        "insert into notifications (user_id, kind, title, created_at, updated_at, group_key, count) \
         values ($1::uuid, 'channel-message', 'unread group, active yesterday', \
                 now() - interval '181 days', now() - interval '1 day', 'engine-live:quiet-group', 40) \
         returning id::text",
    )
    .bind(&bob)
    .fetch_one(&pg)
    .await
    .unwrap();
    for id in &aged {
        sqlx::query(
            "insert into inbox_focus_state (user_id, source_type, source_id, snoozed_until) \
             values ($1::uuid, 'notification', $2, now() + interval '1 day')",
        )
        .bind(&bob)
        .bind(id)
        .execute(&pg)
        .await
        .unwrap();
    }
    let gone = retention::retention_pass(&pg).await.unwrap();
    assert!(gone >= 2);
    let left: Vec<String> = sqlx::query_scalar(
        "select title from notifications where id = any($1::uuid[]) order by title",
    )
    .bind(&aged)
    .fetch_all(&pg)
    .await
    .unwrap();
    assert_eq!(left, ["read 30 d", "unread 100 d"]);
    let kept_group: i64 =
        sqlx::query_scalar("select count(*) from notifications where id = $1::uuid")
            .bind(&quiet_group)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(kept_group, 1, "an unread row's age is its last activity");
    let focus: Vec<String> = sqlx::query_scalar(
        "select source_id from inbox_focus_state where user_id = $1::uuid and source_type = 'notification' \
         and source_id = any($2) order by source_id",
    )
    .bind(&bob)
    .bind(&aged)
    .fetch_all(&pg)
    .await
    .unwrap();
    let mut kept = vec![aged[2].clone(), aged[3].clone()];
    kept.sort();
    assert_eq!(
        focus, kept,
        "the focus rows of the deleted rows went with them"
    );
    println!("PASS retention");

    // ── 8. The push prune: an ended session's row goes ─────────────────────
    let (sid_a, sid_b) = ("engine-sess-a-7f31c2", "engine-sess-b-7f31c2");
    for sid in [sid_a, sid_b] {
        let _: () = redis::cmd("SET")
            .arg(format!("sess:{sid}"))
            .arg("{}")
            .arg("EX")
            .arg(600)
            .query_async(&mut conn)
            .await
            .unwrap();
        let _: () = redis::cmd("DEL")
            .arg(retention::session_marker_for_sid(sid))
            .query_async(&mut conn)
            .await
            .unwrap();
    }
    let hash = |sid: &str| talaria_api::session::session_hash(sid);
    for (sid, path) in [(sid_a, "prune-a"), (sid_b, "prune-b")] {
        sqlx::query(
            "insert into push_subscriptions (user_id, endpoint, p256dh, auth, session_hash) \
             values ($1::uuid, $2, 'x', 'y', $3)",
        )
        .bind(&bob)
        .bind(format!("{}/{path}", cap.base))
        .bind(hash(sid))
        .execute(&pg)
        .await
        .unwrap();
    }
    sqlx::query(
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth, last_seen_at) values \
           ($1::uuid, $2, 'x', 'y', now() - interval '31 days')",
    )
    .bind(&bob)
    .bind(format!("{}/prune-unbound", cap.base))
    .execute(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth, session_hash, failures, last_seen_at) \
         values ($1::uuid, $2, 'x', 'y', $3, 10, now() - interval '8 days')",
    )
    .bind(&bob)
    .bind(format!("{}/prune-failing", cap.base))
    .bind(hash(sid_b))
    .execute(&pg)
    .await
    .unwrap();
    let endpoints = |pg: PgPool, bob: String| async move {
        let v: Vec<String> = sqlx::query_scalar(
            "select split_part(endpoint, '/', 4) from push_subscriptions \
             where user_id = $1::uuid and endpoint like '%/prune-%' order by 1",
        )
        .bind(bob)
        .fetch_all(&pg)
        .await
        .unwrap();
        v
    };
    // Both sessions live, neither had a marker: backfilled, both kept.
    let pass = retention::push_prune_pass(&pg, Some(conn.clone()))
        .await
        .unwrap();
    assert!(pass.backfilled >= 2, "{pass:?}");
    assert_eq!(
        endpoints(pg.clone(), bob.clone()).await,
        ["prune-a", "prune-b"]
    );
    let marked: bool = redis::cmd("EXISTS")
        .arg(retention::session_marker_for_sid(sid_a))
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(marked, "the backfill wrote the marker");
    // Session A ends (its key and marker gone): its row goes, B's stays.
    let _: () = redis::cmd("DEL")
        .arg(format!("sess:{sid_a}"))
        .arg(retention::session_marker_for_sid(sid_a))
        .query_async(&mut conn)
        .await
        .unwrap();
    let pass = retention::push_prune_pass(&pg, Some(conn.clone()))
        .await
        .unwrap();
    assert_eq!(pass.ended, 1, "{pass:?}");
    assert_eq!(endpoints(pg.clone(), bob.clone()).await, ["prune-b"]);
    // Without Redis the session pass never guesses.
    let pass = retention::push_prune_pass(&pg, None).await.unwrap();
    assert!(pass.sessions_skipped);
    assert_eq!(endpoints(pg.clone(), bob.clone()).await, ["prune-b"]);
    let _: () = redis::cmd("DEL")
        .arg(format!("sess:{sid_b}"))
        .arg(retention::session_marker_for_sid(sid_b))
        .query_async(&mut conn)
        .await
        .unwrap();
    println!("PASS push prune");

    // ── 9. The per-device test ─────────────────────────────────────────────
    let before = cap.on("/ada-ok").len();
    let t = talaria_api::push::send_test(&ada, &ada_dev).await.unwrap();
    assert_eq!(t.status, Some(201), "{t:?}");
    let hits = cap
        .wait("/ada-ok", before + 1, Duration::from_secs(3))
        .await;
    let p = browser.decrypt(&hits.last().unwrap().body);
    assert_eq!(p["kind"], "test");
    assert_eq!(p["title"], "Talaria test notification");
    assert!(
        p["body"]
            .as_str()
            .unwrap()
            .ends_with(": this device gets alerts")
    );
    assert_eq!(
        talaria_api::push::send_test(&ada, &bob_dev)
            .await
            .unwrap_err(),
        talaria_api::push::TestError::NotFound,
        "another person's device is not found"
    );
    println!("PASS send_test");

    // ── 10. The VAPID cache: one unseal for fifty deliveries ───────────────
    let u0 = talaria_api::push::vapid_unseals();
    let before = cap.on("/ada-ok").len();
    for i in 0..50 {
        file(&pg, &ada, "dm", &format!("burst {i}"), &m).await;
    }
    let hits = cap
        .wait("/ada-ok", before + 50, Duration::from_secs(20))
        .await;
    assert_eq!(hits.len(), before + 50);
    let u1 = talaria_api::push::vapid_unseals();
    assert!(u1 - u0 <= 1, "50 deliveries unsealed {} time(s)", u1 - u0);
    let k = |h: &Hit| {
        h.headers["authorization"]
            .split("k=")
            .nth(1)
            .unwrap()
            .to_string()
    };
    let old_k = k(hits.last().unwrap());
    sqlx::query("delete from app_settings where key = $1")
        .bind(VAPID_KEY)
        .execute(&pg)
        .await
        .unwrap();
    file(&pg, &ada, "dm", "after a re-mint", &m).await;
    let hits = cap
        .wait("/ada-ok", before + 51, Duration::from_secs(5))
        .await;
    assert_ne!(
        k(hits.last().unwrap()),
        old_k,
        "a deleted keypair is re-minted, and used"
    );
    assert_eq!(talaria_api::push::vapid_unseals(), u1 + 1);
    println!("PASS vapid cache");

    // ── 11. A person with no zone reads the workspace's clock ──────────────
    // Working hours that bracket Tokyo's hour (UTC+9 all year) and exclude
    // UTC's: in a Tokyo workspace, a no-zone person is inside them.
    let cy = person(&pg, "cy").await;
    let tokyo = (chrono::Utc::now() + chrono::Duration::hours(9))
        .format("%H")
        .to_string()
        .parse::<u32>()
        .unwrap();
    let schedule = json!({ "dnd": { "schedule": {
        "enabled": true, "days": [0,1,2,3,4,5,6],
        "start": format!("{:02}:00", (tokyo + 23) % 24),
        "end": format!("{:02}:00", (tokyo + 2) % 24),
    } } });
    set_prefs(&pg, &cy, schedule).await;
    sqlx::query(
        "insert into app_settings (key, value) values ('brief_config', '{\"timeZone\": \"Asia/Tokyo\"}')",
    )
    .execute(&pg)
    .await
    .unwrap();
    // The workspace zone is held for 10 s.
    tokio::time::sleep(Duration::from_secs(11)).await;
    let (_, zone) = talaria_api::notify::prefs_and_zone(&pg, &cy).await;
    assert_eq!(zone, "Asia/Tokyo", "no profile zone → the workspace's");
    let id = file(&pg, &cy, "dm", "inside Tokyo's working hours", &m).await;
    assert_eq!(row(&pg, &id).await.alert.as_deref(), Some("banner"));
    // A zone of their own wins: in UTC the same hours are outside.
    exec(
        &pg,
        "update users set timezone = 'UTC' where id = $1::uuid",
        &cy,
    )
    .await;
    let (_, zone) = talaria_api::notify::prefs_and_zone(&pg, &cy).await;
    assert_eq!(zone, "UTC");
    let id = file(&pg, &cy, "dm", "outside UTC's working hours", &m).await;
    assert_eq!(row(&pg, &id).await.alert.as_deref(), Some("feed"));
    println!("PASS workspace zone");
}

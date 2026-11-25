// Live-DB proof of /api/home/queues: it answers the same queues /api/home
// answers for the same person, and it never reaches the admin glance around
// them — the alert count (a hook set here to a counting closure) or the cost
// overview (whose table is held under an exclusive lock while the queues
// answer, so a read of it would hang). The handlers are the real ones, behind
// a real session. `#[ignore]`d — they need Postgres and Redis — and CI's
// live-tests job runs them (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test -p talaria-routes-comms --lib -- --ignored home_queues_live

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use futures_util::future::BoxFuture;
use serde_json::Value;
use sqlx::PgPool;
use talaria_session::{SessionUser, create_session};
use talaria_state::AppState;

/// Every call the alert hook took in this test binary.
static ALERT_CALLS: AtomicUsize = AtomicUsize::new(0);

async fn state() -> AppState {
    let cfg = talaria_config::Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the live tests"),
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into()),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        "5274".into(),
    )
    .expect("test config");
    let pg = PgPool::connect(&cfg.database_url)
        .await
        .expect("DATABASE_URL reachable");
    AppState::new(pg, Arc::new(cfg))
}

async fn cleanup(pg: &PgPool, tag: &str) {
    // The cascade takes the boards they own, their tickets and memberships.
    sqlx::query("delete from users where sub like $1 || ':%'")
        .bind(format!("home-queues-live:{tag}"))
        .execute(pg)
        .await
        .unwrap();
}

/// A person with `role`, and the headers their browser sends.
async fn person(state: &AppState, tag: &str, who: &str, role: &str) -> (String, HeaderMap) {
    let sub = format!("home-queues-live:{tag}:{who}");
    let id: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, $2, $3) returning id::text",
    )
    .bind(&sub)
    .bind(who)
    .bind(role)
    .fetch_one(&state.pg)
    .await
    .expect("the user");
    let sid = create_session(
        state,
        &SessionUser {
            id: id.clone(),
            sub: sub.clone(),
            email: Some(sub),
            name: Some(who.to_string()),
            picture: None,
            role: role.to_string(),
            provider: "password".into(),
        },
    )
    .await
    .expect("REDIS_URL reachable");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("{}={sid}", talaria_session::SESSION_COOKIE)
            .parse()
            .unwrap(),
    );
    (id, headers)
}

/// A board `owner` owns; `member` (when given) can see it.
async fn board(pg: &PgPool, owner: &str, member: Option<&str>, name: &str) -> String {
    let id: String = sqlx::query_scalar(
        "insert into boards (name, owner_id) values ($1, $2::uuid) returning id::text",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(pg)
    .await
    .expect("the board");
    if let Some(m) = member {
        sqlx::query("insert into board_members (board_id, user_id) values ($1::uuid, $2::uuid)")
            .bind(&id)
            .bind(m)
            .execute(pg)
            .await
            .expect("the membership");
    }
    id
}

async fn ticket(pg: &PgPool, board: &str, title: &str, status: &str, archived: bool) {
    sqlx::query(
        "insert into tasks (board_id, title, status, archived_at) \
         values ($1::uuid, $2, $3, case when $4 then now() end)",
    )
    .bind(board)
    .bind(title)
    .bind(status)
    .bind(archived)
    .execute(pg)
    .await
    .expect("the ticket");
}

async fn body(res: Response) -> Value {
    assert_eq!(res.status(), StatusCode::OK);
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

fn titles(queues: &Value, bucket: &str) -> Vec<String> {
    queues[bucket]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore]
async fn home_queues_live_answers_homes_queues_and_never_the_admin_glance() {
    let state = state().await;
    // A name no other run's rows share.
    let tag: String = sqlx::query_scalar("select md5(random()::text || clock_timestamp()::text)")
        .fetch_one(&state.pg)
        .await
        .unwrap();
    let hook = talaria_home::COMPUTE_ALERT_COUNT.set(Arc::new(
        |_: AppState, _: String| -> BoxFuture<'static, i32> {
            ALERT_CALLS.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { 0 })
        },
    ));
    assert!(
        hook.is_ok(),
        "nothing else in this binary sets the alert hook"
    );

    let (admin, headers) = person(&state, &tag, "admin", "admin").await;
    let (stranger, _) = person(&state, &tag, "stranger", "member").await;
    let seen = board(&state.pg, &admin, Some(&admin), &format!("hq-{tag}-seen")).await;
    let unseen = board(&state.pg, &stranger, None, &format!("hq-{tag}-unseen")).await;
    ticket(&state.pg, &seen, "needs triage", "inbox", false).await;
    ticket(&state.pg, &seen, "needs sign-off", "quality_review", false).await;
    ticket(&state.pg, &seen, "is blocked", "blocked", false).await;
    ticket(&state.pg, &seen, "in hand", "in_progress", false).await;
    ticket(&state.pg, &seen, "archived triage", "inbox", true).await;
    ticket(&state.pg, &unseen, "someone else's triage", "inbox", false).await;

    // The cost overview reads usage_events: held exclusively, any read of it
    // waits for this transaction. The queues must answer anyway.
    let mut lock = state.pg.begin().await.unwrap();
    sqlx::query("lock table usage_events in access exclusive mode")
        .execute(&mut *lock)
        .await
        .unwrap();
    let before = ALERT_CALLS.load(Ordering::SeqCst);
    let queues = tokio::time::timeout(
        Duration::from_secs(20),
        super::get(State(state.clone()), headers.clone()),
    )
    .await
    .expect("the queues answer while the cost table is locked")
    .unwrap();
    let queues = body(queues).await;
    assert_eq!(
        ALERT_CALLS.load(Ordering::SeqCst),
        before,
        "the queues never reach the alert count"
    );
    // The trap is real: /api/home for the same admin reaches both — the hook
    // counts, and the cost read waits on the lock.
    let held = tokio::time::timeout(
        Duration::from_secs(2),
        super::super::home::get(State(state.clone()), headers.clone()),
    )
    .await;
    assert!(
        held.is_err(),
        "/api/home's cost overview waited on the lock"
    );
    assert_eq!(
        ALERT_CALLS.load(Ordering::SeqCst),
        before + 1,
        "/api/home asked the alert hook"
    );
    lock.rollback().await.unwrap();

    let home = body(
        super::super::home::get(State(state.clone()), headers.clone())
            .await
            .unwrap(),
    )
    .await;
    let obj = queues.as_object().unwrap();
    assert_eq!(
        obj.keys().collect::<Vec<_>>(),
        ["queues"],
        "the queues and nothing else"
    );
    assert_eq!(
        queues["queues"], home["queues"],
        "the same queues /api/home answers"
    );
    let q = &queues["queues"];
    assert_eq!(titles(q, "triage"), ["needs triage"]);
    assert_eq!(titles(q, "review"), ["needs sign-off"]);
    assert_eq!(titles(q, "blocked"), ["is blocked"]);
    for bucket in ["triage", "review", "blocked"] {
        assert_eq!(q[bucket]["count"], 1, "{bucket}");
    }

    cleanup(&state.pg, &tag).await;
}

#[tokio::test]
#[ignore]
async fn home_queues_live_refuses_without_a_session() {
    let state = state().await;
    let res = super::get(State(state), HeaderMap::new()).await;
    let status = match res {
        Ok(r) | Err(r) => r.status(),
    };
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

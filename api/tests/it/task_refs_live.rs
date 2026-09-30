// task_refs_live (see main.rs): the it/ binary's shared support::mod supplies
// the helpers; this file carries only what is specific to the ticket-address
// door.
//
// Live-DB proof of the ticket-address door (cargo test -- --ignored). The
// routes under /api/tasks/{id} take the row uuid OR the ref the board shows
// (`TALA-30`); every guarantee this file pins is one a unit test cannot vouch
// for because it IS a router gate riding a real lookup:
//
//   uuid     → 200, and the ref resolves to the SAME task
//   junk     → 400, never a 5xx — the string never touched the database
//   unknown  → 404 whose body NAMES the ref (no ticket matches TALA-99999)
//   shared   → 409 — two boards own the prefix+number, so it names the
//              collision instead of picking one
//
// Each test drives the REAL router (routes::router) with a minted session,
// because the resolution and its error envelope live on the route, not in the
// parse helper a unit test can reach. The secondary surface
// (/api/tasks/{id}/comments) rides the same resolver — one route proves the
// shared door carried; asserting it twice would test nothing twice.
//
// House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test -p talaria-api --test it task_refs_live:: -- --ignored

use axum::body::Body;
use axum::http::Request;
use sqlx::PgPool;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

/// Per-test sweep tags: the tests run CONCURRENTLY against the one rig, and
/// one test's cleanup deleting the prefix out from under another's is the
/// failure this layout exists to prevent. The sweep direction matches
/// production deletes: the users cascade boards → tasks.
const TAG_ADDRESS: &str = "task-refs-a";
const TAG_SHARED: &str = "task-refs-s";
const TAG_COMMENTS: &str = "task-refs-c";

async fn app_state() -> AppState {
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL (source ui/.env)"),
        std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)"),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

/// Delete the tag's rows. Run at the START of a test — a failed previous
/// run's leftovers would otherwise shadow the next one's assertions.
async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where email like $1")
        .bind(format!("{tag}-%@link-test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

/// A member user row with a stable per-tag email the sweep matches.
async fn owner(pg: &PgPool, tag: &str) -> SessionUser {
    let email = format!("{tag}-owner@link-test.invalid");
    let sub = format!("{tag}:{email}");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') \
         returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .bind("Task Refs Live Owner")
    .fetch_one(pg)
    .await
    .unwrap();
    SessionUser {
        id,
        sub,
        email: Some(email),
        name: Some("Task Refs Live Owner".into()),
        picture: None,
        role: "member".into(),
        provider: "google".into(),
    }
}

/// A board whose prefix is the caller's, plus the board_members row the app
/// provisions (access reads membership, not owner_id — display truth).
async fn board(pg: &PgPool, user_id: &str, prefix: &str, name: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id, ticket_prefix) \
         values ($2, $1::uuid, $3) returning id::text",
    )
    .bind(user_id)
    .bind(name)
    .bind(prefix)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&id)
    .bind(user_id)
    .execute(pg)
    .await
    .unwrap();
    id
}

/// A ticket with an explicit number, so `PREFIX-<no>` is a real address.
async fn task(pg: &PgPool, board_id: &str, ticket_no: i32, title: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, title, status, priority, ticket_no, created_by) \
         values ($1::uuid, $3, 'todo', 'medium', $2, 'live-test') returning id::text",
    )
    .bind(board_id)
    .bind(ticket_no)
    .bind(title)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// GET through the REAL router with a minted session cookie — the stack a
/// browser rides, status and body, because the assertion is about the
/// envelope the caller reads, not the query under it.
async fn get(state: &AppState, sid: &str, path: &str) -> (u16, String) {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("talaria_session={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// The uuid is an address and so is the ref: both 200, and BOTH name the
/// same ticket — a ref that resolved to the wrong row would be worse than
/// the 500 this door replaced. Junk is a 400 teaching the fix, never a 5xx,
/// and an unknown-but-shaped ref is a 404 that NAMES the ref, because "not
/// found" on an address copied off a card is not actionable.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn uuid_and_ref_both_fetch_the_task_and_misses_say_which() {
    let state = app_state().await;
    sweep(&state.pg, TAG_ADDRESS).await;
    let owner = owner(&state.pg, TAG_ADDRESS).await;
    let b = board(&state.pg, &owner.id, "TALA36A", "address board").await;
    let task_id = task(&state.pg, &b, 30, "The acceptance ticket").await;
    let sid = create_session(&state, &owner).await.unwrap();

    // The uuid, straight through.
    let (status, body) = get(&state, &sid, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(&task_id),
        "the uuid read must be the ticket itself"
    );

    // The ref the board shows, resolving to the SAME row.
    let (status, body) = get(&state, &sid, "/api/tasks/TALA36A-30").await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(&task_id),
        "the ref must resolve to the same ticket, got: {body}"
    );
    assert!(
        body.contains("TALA36A-30"),
        "the ref rides the body as ticketRef"
    );

    // Junk: neither a uuid nor PREFIX-<n> shaped. A 400 — the fix is the
    // caller's — and never a 5xx, the shape that predated this door.
    for junk in ["nope", "TALA36A-", "30", "%2F..%2Fetc"] {
        let (status, body) = get(&state, &sid, &format!("/api/tasks/{junk}")).await;
        assert_eq!(status, 400, "junk {junk:?} answered {status}: {body}");
        assert_eq!(
            body,
            r#"{"error":"task id must be a UUID or a ticket ref like TALA-30"}"#
        );
    }

    // Unknown uuid: shaped, looked up, absent — the honest 404.
    let unknown_uuid = "00000000-0000-4000-8000-000000000000";
    let (status, body) = get(&state, &sid, &format!("/api/tasks/{unknown_uuid}")).await;
    assert_eq!(status, 404, "{body}");

    // Unknown ref: a real lookup that found no such ticket, and the body
    // NAMES the ref so the caller knows which address to stop trying.
    let (status, body) = get(&state, &sid, "/api/tasks/TALA36A-99999").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body, r#"{"error":"no ticket matches TALA36A-99999"}"#);

    sweep(&state.pg, TAG_ADDRESS).await;
}

/// Two boards sharing a prefix+number is nobody's guess to make: the door
/// answers 409 naming the collision rather than picking one board's ticket.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn a_ref_two_boards_share_is_a_409_not_a_guess() {
    let state = app_state().await;
    sweep(&state.pg, TAG_SHARED).await;
    let owner = owner(&state.pg, TAG_SHARED).await;
    let left = board(&state.pg, &owner.id, "TALA36B", "shared left").await;
    let right = board(&state.pg, &owner.id, "TALA36B", "shared right").await;
    let a = task(&state.pg, &left, 7, "left's seven").await;
    let b = task(&state.pg, &right, 7, "right's seven").await;
    let sid = create_session(&state, &owner).await.unwrap();

    let (status, body) = get(&state, &sid, "/api/tasks/TALA36B-7").await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        body,
        r#"{"error":"that ticket ref matches more than one board — pass the ticket id"}"#
    );
    // And the 409 is not a dodge: either row's uuid still reads fine.
    let (status, _) = get(&state, &sid, &format!("/api/tasks/{a}")).await;
    assert_eq!(status, 200);
    let (status, _) = get(&state, &sid, &format!("/api/tasks/{b}")).await;
    assert_eq!(status, 200);

    sweep(&state.pg, TAG_SHARED).await;
}

/// The same resolver carries the secondary doors — the comments list proves
/// the shared carry without re-asserting the whole matrix.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn the_comments_door_carries_the_same_address() {
    let state = app_state().await;
    sweep(&state.pg, TAG_COMMENTS).await;
    let owner = owner(&state.pg, TAG_COMMENTS).await;
    let b = board(&state.pg, &owner.id, "TALA36C", "comments board").await;
    let _task_id = task(&state.pg, &b, 12, "the commented ticket").await;
    let sid = create_session(&state, &owner).await.unwrap();

    let (status, body) = get(&state, &sid, "/api/tasks/TALA36C-12/comments").await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = get(&state, &sid, "/api/tasks/definitely-not-an-id/comments").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body,
        r#"{"error":"task id must be a UUID or a ticket ref like TALA-30"}"#
    );

    let (status, body) = get(&state, &sid, "/api/tasks/TALA36C-999/comments").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body, r#"{"error":"no ticket matches TALA36C-999"}"#);

    sweep(&state.pg, TAG_COMMENTS).await;
}

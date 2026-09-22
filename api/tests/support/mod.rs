// Shared support for the live (`#[ignore]`d) integration tests.
//
// WHY THIS FILE EXISTS: every one of these helpers was copied per test binary —
// 19 `pool()`/`pg()` pairs, 6 `fabricate_user`, 3 `person`, 3
// `sweep_user_rows`, 3 `app_state`, and the same DATABASE_URL expect string 25
// times. An integration test is its own crate, so a helper shared between two
// of them HAD to be copied; that is the whole reason the copies exist, and it
// is also why they drifted (four spellings of the expect string, three of the
// fixture prefix). The copy is not the bug — the bug is that the copy is
// invisible. This module makes it one file.
//
// Every helper here is a straight lift: same SQL, same ordering, same panic
// messages. The per-binary variation (the fixture prefix) is a PARAMETER, not
// a second function.
//
// Integration tests compile this module per binary, so each crate sees helpers
// it does not use; that is what the allow is for.
#![allow(dead_code)]

use sqlx::PgPool;

/// The panic message for a missing DATABASE_URL, spelled once. It says where
/// to get one because the alternative is a test that "cannot find" a database
/// nobody told you to start.
pub const DATABASE_URL_EXPECT: &str =
    "set DATABASE_URL (source ui/.env) to run the ignored live tests";

/// A pool against the developer's stack.
pub async fn pg() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect(DATABASE_URL_EXPECT);
    PgPool::connect(&url).await.expect("connect")
}

/// Set the boot-injected seams a test binary needs. A test binary never runs
/// `register_all` (it is the scheduler's arming path, and wants an AppState),
/// so any OnceLock edge a test exercises has to be set here, exactly the way
/// `register_all` sets it in production — same closure, same target. Today
/// that is the attribution ladder's CONVERSATION_OWNER (the chatter rung is
/// silently skipped when it is unset, which is precisely how it went dead in
/// production for as long as it did).
pub fn wire_boot_seams() {
    let _ = talaria_api::attribution::CONVERSATION_OWNER.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_api::conversations::conversation_owner(&pg, &id).await })
    }));
    let _ = talaria_workchains::GET_TASK.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_tasks::get_task(&pg, &id).await })
    }));
    let _ = talaria_inbox_focus::GET_TASK.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_tasks::get_task(&pg, &id).await })
    }));
}

/// A member user row, with the sub/email/name the caller names.
pub async fn person(pg: &PgPool, sub: &str, email: &str, name: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') returning id::text",
    )
    .bind(sub)
    .bind(email)
    .bind(name)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// A throwaway user whose `sub` carries `<prefix>:<tag>:<uuid>`, so the
/// binary's own sweep pattern (`sub like '<prefix>:<tag>:%'`) can clean up
/// after a failed run. Returns the new row's id.
pub async fn fabricate_user(pg: &PgPool, prefix: &str, tag: &str) -> String {
    let sub = format!("{prefix}:{tag}:{}", uuid::Uuid::new_v4());
    sqlx::query("insert into users (sub) values ($1)")
        .bind(&sub)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query_scalar("select id::text from users where sub = $1")
        .bind(&sub)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// Delete the rows `fabricate_user` made for this prefix+tag. Run it at the
/// START of a test: a failed previous run's leftovers would otherwise shadow
/// the next one's assertions.
pub async fn sweep_user_rows(pg: &PgPool, prefix: &str, tag: &str) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{prefix}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// The app's state, built from the ambient env with a DEAD redis
/// (`redis://127.0.0.1:1`) — the shape the tests that exercise a route's
/// Postgres path use, where a live redis would only add a second dependency to
/// the failure they are actually driving.
pub async fn app_state() -> talaria_state::AppState {
    let cfg = talaria_config::Config::from_parts(
        std::env::var("DATABASE_URL").unwrap_or_default(),
        "redis://127.0.0.1:1".into(),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    talaria_state::AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

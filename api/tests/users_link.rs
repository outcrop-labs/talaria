// Live-DB proof of the sign-in email link (cargo test -- --ignored). The
// link is one UPDATE whose correctness is a WHERE clause — a sign-in that
// forks a second row for a claimed email does not fail a unit test, it
// silently hands the person two identities. These tests drive upsert_user
// and link_by_email against the real table: the same-email link that keeps
// the role, the brand-new member, the re-sign-in that stays one row, the
// unmerged fork whose sub is never stolen, and the claim's promotion.
// claim_admin itself is not driven end-to-end: its gate is instance-wide
// (no admin anywhere), which a shared dev database cannot offer — the
// behavior it gained IS link_by_email(promote), driven here inside the same
// statement the claim runs in its transaction. House rule: #[ignore]d,
// never CI.
//
//   DATABASE_URL=postgres://… cargo test --test users_link -- --ignored

use sqlx::postgres::PgPool;
use std::time::Duration;
use talaria_api::password_accounts::verify_password_login;
use talaria_api::users::{Identity, link_by_email, upsert_user};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Every row this suite fabricates lives under one domain, so a crashed run
/// cannot leak users into the next (credentials and memberships ride the
/// cascades).
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where lower(email) like '%@link-test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

/// (id, sub, email, name, role) for the one row an email names — panics
/// when the email does not name exactly one, which is the fork this suite
/// exists to catch.
async fn one_row(
    pg: &PgPool,
    email: &str,
) -> (String, String, Option<String>, Option<String>, String) {
    let rows: Vec<(String, String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select id::text, sub, email, name, role from users where lower(email) = lower($1)",
    )
    .bind(email)
    .fetch_all(pg)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one row for {email}, found {}",
        rows.len()
    );
    rows.into_iter().next().unwrap()
}

async fn seed_user(pg: &PgPool, sub: &str, email: &str, name: &str, role: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, $4) returning id::text",
    )
    .bind(sub)
    .bind(email)
    .bind(name)
    .bind(role)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_google_sign_in_links_to_the_same_email_admin() {
    let pg = pool().await;
    cleanup(&pg).await;
    seed_user(
        &pg,
        "password:one@link-test.invalid",
        "one@link-test.invalid",
        "One Admin",
        "admin",
    )
    .await;

    // Mixed case on purpose: the link compares lower(email) to lower(email).
    let row = upsert_user(
        &pg,
        &Identity {
            sub: "google:1".into(),
            email: Some("One@Link-Test.Invalid".into()),
            name: Some("Googler One".into()),
            picture: Some("https://example.test/one.png".into()),
        },
    )
    .await
    .unwrap();

    let (id, sub, email, name, role) = one_row(&pg, "one@link-test.invalid").await;
    assert_eq!(role, "admin", "the link must never touch the role");
    assert_eq!(sub, "google:1", "the row adopts the Google sub");
    assert_eq!(email.as_deref(), Some("One@Link-Test.Invalid"));
    // The row already carries a name the person chose, so the link keeps it —
    // the same guard the on-conflict path has always run.
    assert_eq!(name.as_deref(), Some("One Admin"));
    assert_eq!(row.0, id, "the returned row is the linked row");
    assert_eq!(row.5, "admin", "the session sees the admin role");
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_google_sign_in_without_a_same_email_row_creates_a_member() {
    let pg = pool().await;
    cleanup(&pg).await;

    upsert_user(
        &pg,
        &Identity {
            sub: "google:2".into(),
            email: Some("two@link-test.invalid".into()),
            name: Some("Googler Two".into()),
            picture: None,
        },
    )
    .await
    .unwrap();

    let (_, sub, _, _, role) = one_row(&pg, "two@link-test.invalid").await;
    assert_eq!(sub, "google:2");
    assert_eq!(role, "member");
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_re_sign_in_is_still_one_row() {
    let pg = pool().await;
    cleanup(&pg).await;
    seed_user(
        &pg,
        "password:three@link-test.invalid",
        "three@link-test.invalid",
        "Three Admin",
        "admin",
    )
    .await;
    let identity = Identity {
        sub: "google:3".into(),
        email: Some("three@link-test.invalid".into()),
        name: Some("Three".into()),
        picture: Some("https://example.test/three.png".into()),
    };
    upsert_user(&pg, &identity).await.unwrap();
    let (first_seen,): (i64,) = sqlx::query_as(
        "select (extract(epoch from last_seen_at) * 1000000)::bigint from users where sub = 'google:3'",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    upsert_user(&pg, &identity).await.unwrap();

    let (_, sub, _, _, role) = one_row(&pg, "three@link-test.invalid").await;
    assert_eq!(sub, "google:3");
    assert_eq!(role, "admin");
    let (second_seen,): (i64,) = sqlx::query_as(
        "select (extract(epoch from last_seen_at) * 1000000)::bigint from users where sub = 'google:3'",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        second_seen > first_seen,
        "the re-sign-in refreshes last_seen_at"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_link_never_steals_a_sub_another_row_holds() {
    let pg = pool().await;
    cleanup(&pg).await;
    // The unmerged fork: an admin row for the email plus a member row that
    // already holds the incoming Google sub. The sign-in must land on the
    // ADMIN — the boot migration merges the fork, not the sign-in.
    seed_user(
        &pg,
        "password:four@link-test.invalid",
        "four@link-test.invalid",
        "Four Admin",
        "admin",
    )
    .await;
    let fork_id = seed_user(
        &pg,
        "google:4",
        "four@link-test.invalid",
        "Four Fork",
        "member",
    )
    .await;

    let row = upsert_user(
        &pg,
        &Identity {
            sub: "google:4".into(),
            email: Some("four@link-test.invalid".into()),
            name: Some("Four".into()),
            picture: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(row.5, "admin", "the sign-in lands on the admin");
    assert_eq!(
        row.1, "password:four@link-test.invalid",
        "the admin keeps its sub"
    );

    let (fork_sub,): (String,) = sqlx::query_as("select sub from users where id = $1::uuid")
        .bind(&fork_id)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(fork_sub, "google:4", "the fork is left for the migration");
    let (n,): (i64,) =
        sqlx::query_as("select count(*) from users where lower(email) = 'four@link-test.invalid'")
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(n, 2, "merging is not the sign-in's job");
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_claim_promotion_upgrades_the_same_email_row_in_place() {
    let pg = pool().await;
    cleanup(&pg).await;
    let member_id = seed_user(
        &pg,
        "google:five@link-test.invalid",
        "five@link-test.invalid",
        "Five Member",
        "member",
    )
    .await;

    // What claim_admin now runs inside its transaction: link + promote.
    let claimed = link_by_email(
        &pg,
        &Identity {
            sub: "password:five@link-test.invalid".into(),
            email: Some("five@link-test.invalid".into()),
            name: Some("Five".into()),
            picture: None,
        },
        true,
    )
    .await
    .unwrap()
    .expect("the same-email row links");
    assert_eq!(
        claimed.0, member_id,
        "the member row is upgraded, not forked"
    );
    assert_eq!(claimed.5, "admin");

    let (id, sub, _, _, role) = one_row(&pg, "five@link-test.invalid").await;
    assert_eq!(id, member_id);
    assert_eq!(sub, "password:five@link-test.invalid");
    assert_eq!(role, "admin");
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_claim_still_writes_the_credential_on_the_promoted_row() {
    let pg = pool().await;
    cleanup(&pg).await;
    seed_user(
        &pg,
        "google:six@link-test.invalid",
        "six@link-test.invalid",
        "Six Member",
        "member",
    )
    .await;
    // The credential insert the claim performs after linking, keyed on the
    // promoted row's id — same statement claim_admin runs.
    let hash = talaria_api::password::hash_password("correct horse");
    let (id,): (String,) =
        sqlx::query_as("select id::text from users where sub = 'google:six@link-test.invalid'")
            .fetch_one(&pg)
            .await
            .unwrap();
    sqlx::query(
        "insert into user_password_credentials (user_id, email, password_hash) \
         values ($1::uuid, $2, $3) \
         on conflict (user_id) do update set \
           email = excluded.email, password_hash = excluded.password_hash, updated_at = now()",
    )
    .bind(&id)
    .bind("six@link-test.invalid")
    .bind(&hash)
    .execute(&pg)
    .await
    .unwrap();

    let li = verify_password_login(&pg, "six@link-test.invalid", "correct horse")
        .await
        .unwrap()
        .expect("the credential verifies");
    assert_eq!(li.sub, "google:six@link-test.invalid");

    // The password sign-in path: upsert with the row's own sub must not fork.
    upsert_user(
        &pg,
        &Identity {
            sub: li.sub,
            email: Some(li.email),
            name: li.name,
            picture: li.picture,
        },
    )
    .await
    .unwrap();
    one_row(&pg, "six@link-test.invalid").await;
}

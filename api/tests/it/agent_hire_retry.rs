// Live-DB proof of the agent-hire Retry's database half (cargo test -- --ignored).
//
// The route's refusals are pure functions with unit tests of their own; what
// only Postgres can vouch for is the rest: who `may_act` lets through (Start's
// manager rule while the agent exists, the hirer WITH `agents.manage` once it
// is gone), the `newest` column that refuses a stale row, the derived retry id
// and the 409 a second retry of one hire meets, and the roster's listing
// clause that keeps an unanswered failure however old.
//
//   DATABASE_URL=postgres://… cargo test --test it agent_hire_retry:: -- --ignored

use crate::support::pg;
use axum::http::StatusCode;
use serde_json::json;
use sqlx::postgres::PgPool;
use talaria_routes_fleet::fleet::fleet_hires::HIRES_LISTED;
use talaria_routes_fleet::fleet::fleet_hires_id_retry::{
    may_act, plan_retry, read_hire, retried_already,
};

const PREFIX: &str = "hire-retry-test";

/// Each test owns its own tag and cleans up only its own rows: the tests in
/// this file run concurrently.
async fn cleanup(pg: &PgPool, tag: &str) {
    let like = format!("{PREFIX}-{tag}-%");
    sqlx::query("delete from runs where kind = 'agent-hire' and input->>'slug' like $1")
        .bind(&like)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(&like)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// A user; `manage` grants them `agents.manage` the way an admin's per-user
/// override does.
async fn user(pg: &PgPool, tag: &str, who: &str, role: &str, manage: bool) -> String {
    let sub = format!("{PREFIX}:{tag}:{who}");
    let id: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, $2, $3) returning id::text",
    )
    .bind(&sub)
    .bind(who)
    .bind(role)
    .fetch_one(pg)
    .await
    .unwrap();
    if manage {
        sqlx::query(
            "insert into user_permissions (user_id, perm, allowed) values ($1::uuid, 'agents.manage', true)",
        )
        .bind(&id)
        .execute(pg)
        .await
        .unwrap();
    }
    id
}

fn slug(tag: &str, name: &str) -> String {
    format!("{PREFIX}-{tag}-{name}")
}

/// The agent's def, managed by `manager` — what a hire's create stage leaves.
async fn agent(pg: &PgPool, slug: &str, manager: &str) -> String {
    let id: String = sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, $1, $2, $1) returning id::text",
    )
    .bind(slug)
    .bind(format!("{slug}-model"))
    .fetch_one(pg)
    .await
    .unwrap();
    talaria_agent_managers::add_manager(pg, &id, manager, Some(manager))
        .await
        .unwrap();
    id
}

/// An agent-hire run of `slug` in `state`, `age` ago (a Postgres interval),
/// owned by `owner`, its input the shape the create route writes.
async fn hire(pg: &PgPool, slug: &str, state: &str, owner: &str, age: &str) -> String {
    sqlx::query_scalar(
        "insert into runs (kind, owner_user_id, subject_type, subject_id, state, phase, input, \
                           created_at, updated_at) \
         values ('agent-hire', $1::uuid, 'agent-hire', $2, $3, '', $4, \
                 now() - $5::interval, now() - $5::interval) \
         returning id::text",
    )
    .bind(owner)
    .bind(slug)
    .bind(state)
    .bind(json!({
        "slug": slug, "department": slug, "displayName": slug, "start": true,
        "actor": "hirer@example.com", "managerUserId": owner,
    }))
    .bind(age)
    .fetch_one(pg)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn may_act_is_start_s_rule_and_a_deleted_agent_needs_its_hirer_with_agents_manage() {
    let pg = pg().await;
    let tag = "may";
    cleanup(&pg, tag).await;
    let admin = user(&pg, tag, "admin", "admin", false).await;
    let manager = user(&pg, tag, "manager", "member", false).await;
    let fleet_hand = user(&pg, tag, "fleet-hand", "member", true).await;
    let bystander = user(&pg, tag, "bystander", "member", false).await;
    let hirer = user(&pg, tag, "hirer", "member", true).await;
    let lapsed = user(&pg, tag, "lapsed", "member", false).await;
    let s = slug(tag, "sloane");
    agent(&pg, &s, &manager).await;

    // The agent exists: Start's rule — its managers and admins, nobody else.
    for (who, id, role, allowed) in [
        ("admin", &admin, "admin", true),
        ("the manager", &manager, "member", true),
        (
            "agents.manage without managing it",
            &fleet_hand,
            "member",
            false,
        ),
        ("a bystander", &bystander, "member", false),
        // The hire's own owner, who manages nothing of it now.
        ("the hire's owner", &hirer, "member", false),
    ] {
        assert_eq!(
            may_act(&pg, id, role, &s, true, Some(&hirer))
                .await
                .unwrap(),
            allowed,
            "{who}, agent present"
        );
    }

    // The agent was deleted: a retry would create it again — the hirer only
    // with `agents.manage` still in hand, or an admin.
    for (who, id, role, owner, allowed) in [
        ("admin", &admin, "admin", &hirer, true),
        (
            "the hirer, still holding agents.manage",
            &hirer,
            "member",
            &hirer,
            true,
        ),
        (
            "the hirer, agents.manage since lost",
            &lapsed,
            "member",
            &lapsed,
            false,
        ),
        (
            "agents.manage, not the hirer",
            &fleet_hand,
            "member",
            &hirer,
            false,
        ),
        ("a bystander", &bystander, "member", &hirer, false),
    ] {
        assert_eq!(
            may_act(&pg, id, role, &s, false, Some(owner))
                .await
                .unwrap(),
            allowed,
            "{who}, agent deleted"
        );
    }
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn only_the_newest_failed_hire_of_a_handle_plans_a_retry() {
    let pg = pg().await;
    let tag = "newest";
    cleanup(&pg, tag).await;
    let manager = user(&pg, tag, "manager", "member", true).await;
    let fleet_hand = user(&pg, tag, "fleet-hand", "member", true).await;
    let s = slug(tag, "sloane");
    agent(&pg, &s, &manager).await;
    let stale = hire(&pg, &s, "error", &manager, "2 hours").await;
    let newest = hire(&pg, &s, "error", &manager, "1 hour").await;

    // The stale row: a newer hire answers for the agent.
    let (status, why) = plan_retry(&pg, &manager, "member", "m@example.com", &stale)
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT, "{why}");
    assert!(why.contains("newer hire"), "{why}");
    assert!(!read_hire(&pg, &stale).await.unwrap().unwrap().newest);

    // The newest failed hire plans, with the retrier as actor and the
    // derived id.
    let plan = plan_retry(&pg, &manager, "member", "m@example.com", &newest)
        .await
        .unwrap();
    assert_eq!(plan.slug, s);
    assert_eq!(plan.input["actor"], json!("m@example.com"));
    let derived: String =
        sqlx::query_scalar("select md5('agent-hire retry of ' || $1)::uuid::text")
            .bind(&newest)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(plan.retry_id, derived);

    // Someone holding agents.manage who does not manage it: 403.
    let (status, _) = plan_retry(&pg, &fleet_hand, "member", "f@example.com", &newest)
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A hire that did not fail, and one that does not exist.
    let done = slug(tag, "done");
    agent(&pg, &done, &manager).await;
    let finished = hire(&pg, &done, "done", &manager, "1 minute").await;
    let (status, why) = plan_retry(&pg, &manager, "member", "m@example.com", &finished)
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(why.contains("this one is done"), "{why}");
    let (status, _) = plan_retry(
        &pg,
        &manager,
        "member",
        "m@example.com",
        "00000000-0000-0000-0000-000000000000",
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_second_retry_of_one_hire_meets_the_409() {
    let pg = pg().await;
    let tag = "twice";
    cleanup(&pg, tag).await;
    let manager = user(&pg, tag, "manager", "member", true).await;
    let s = slug(tag, "sloane");
    agent(&pg, &s, &manager).await;
    let failed = hire(&pg, &s, "error", &manager, "5 minutes").await;
    let plan = plan_retry(&pg, &manager, "member", "m@example.com", &failed)
        .await
        .unwrap();
    assert_eq!(retried_already(&pg, &plan.retry_id).await, None);

    // The first retry's run, under the derived id…
    let insert = "insert into runs (id, kind, subject_type, subject_id, state, input) \
                  values ($1::uuid, 'agent-hire', 'agent-hire', $2, 'queued', $3)";
    sqlx::query(insert)
        .bind(&plan.retry_id)
        .bind(&s)
        .bind(&plan.input)
        .execute(&pg)
        .await
        .unwrap();
    // …so a second insert of that id fails at the key, and the failed
    // enqueue answers 409, not 500.
    assert!(
        sqlx::query(insert)
            .bind(&plan.retry_id)
            .bind(&s)
            .bind(&plan.input)
            .execute(&pg)
            .await
            .is_err()
    );
    let (status, why) = retried_already(&pg, &plan.retry_id).await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(why.contains("retried already"), "{why}");
    // And the failed row is no longer the newest hire of its handle.
    let (status, _) = plan_retry(&pg, &manager, "member", "m@example.com", &failed)
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT);
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_unanswered_failure_stays_listed_however_old() {
    let pg = pg().await;
    let tag = "listed";
    cleanup(&pg, tag).await;
    let manager = user(&pg, tag, "manager", "member", true).await;

    // An old failure of a draft that is still there, answered by nothing.
    let draft = slug(tag, "draft");
    agent(&pg, &draft, &manager).await;
    let unanswered = hire(&pg, &draft, "error", &manager, "2 hours").await;
    // An old failure whose agent was deleted since.
    let gone = slug(tag, "gone");
    let orphaned = hire(&pg, &gone, "error", &manager, "2 hours").await;
    // An old failure a later hire answered (and that hire is old too).
    let answered_slug = slug(tag, "answered");
    agent(&pg, &answered_slug, &manager).await;
    let answered = hire(&pg, &answered_slug, "error", &manager, "3 hours").await;
    let later = hire(&pg, &answered_slug, "done", &manager, "2 hours").await;
    // A fresh finish and an old live hire.
    let fresh_slug = slug(tag, "fresh");
    agent(&pg, &fresh_slug, &manager).await;
    let fresh = hire(&pg, &fresh_slug, "done", &manager, "2 minutes").await;
    let live_slug = slug(tag, "live");
    agent(&pg, &live_slug, &manager).await;
    let live = hire(&pg, &live_slug, "running", &manager, "3 hours").await;

    let listed: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select id::text from runs where {HIRES_LISTED} and input->>'slug' like $1"
    )))
    .bind(format!("{PREFIX}-{tag}-%"))
    .fetch_all(&pg)
    .await
    .unwrap();
    for (what, id, shown) in [
        ("an unanswered old failure", &unanswered, true),
        ("an old failure of a deleted agent", &orphaned, false),
        ("an old failure a later hire answered", &answered, false),
        ("the old hire that answered it", &later, false),
        ("a hire that finished two minutes ago", &fresh, true),
        ("a live hire, however old", &live, true),
    ] {
        assert_eq!(listed.contains(id), shown, "{what}");
    }
    cleanup(&pg, tag).await;
}

// Live-DB + live-Redis proof of the attribution ladder
// (cargo test -- --ignored). The ladder is three queries and one Redis GET
// whose correctness is ORDER: an owner that outranks a live turn, a chatter
// that outranks a hirer, a legacy caller that gets nothing. Those are exactly
// the properties a unit test cannot catch silently regressing — a reordered
// rung still returns SOMEONE. House rule: #[ignore]d, never CI.
//
// Tests run concurrently, so every test owns its own slug AND its own email
// subdomain; cleanup touches only its own rows.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test --test attribution -- --ignored

use redis::AsyncCommands;
use sqlx::postgres::PgPool;
use talaria_api::agent_auth::{AgentCaller, AgentSubject};
use talaria_api::attribution::responsible_user_for;
use talaria_api::users::{Identity, upsert_user};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

async fn redis() -> redis::aio::ConnectionManager {
    let url = std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)");
    let client = redis::Client::open(url).expect("parse redis url");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("connect redis")
}

/// This test's rows only: its slug's hire runs (runs name agents by bare
/// slug text — no FK — so by hand, first), its agent, its people. A crashed
/// prior run of the SAME test is healed; a sibling test's rows are never
/// touched.
async fn cleanup(pg: &PgPool, slug: &str, subdomain: &str) {
    sqlx::query("delete from runs where subject_id = $1")
        .bind(slug)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where slug = $1")
        .bind(slug)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where lower(email) like $1")
        .bind(format!("%@{subdomain}.attribution-test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

async fn user(pg: &PgPool, email: &str) -> String {
    let (id, ..) = upsert_user(
        pg,
        &Identity {
            sub: format!("attrtest-{email}"),
            email: Some(email.to_string()),
            name: None,
            picture: None,
        },
    )
    .await
    .unwrap();
    id
}

/// A proven subject — id Some means proven, per AgentCaller's contract.
fn caller(model: &str) -> AgentSubject {
    AgentSubject::Caller(AgentCaller {
        id: Some(format!("id-{model}")),
        model: model.to_string(),
        legacy: false,
    })
}

/// The legacy shared-key shape: identified, not proven.
fn legacy(model: &str) -> AgentSubject {
    AgentSubject::Caller(AgentCaller {
        id: None,
        model: model.to_string(),
        legacy: true,
    })
}

/// An agent_defs row. model is slug-department, mirroring production.
async fn agent(pg: &PgPool, slug: &str, owner: Option<&str>) -> String {
    let model = format!("{slug}-ops");
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, owner_user_id) \
         values ($1, 'ops', $2, $1, $3::uuid) \
         on conflict (model) do update set owner_user_id = excluded.owner_user_id \
         returning id::text",
    )
    .bind(slug)
    .bind(&model)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    let _ = id;
    model
}

/// A hire run naming the agent by SLUG — the shape fleet_create writes.
/// `ago_millis` ages the row so LATEST-hire ordering is exercised rather
/// than accidental.
async fn hire(pg: &PgPool, slug: &str, hirer: &str, ago_millis: i64) {
    sqlx::query(
        "insert into runs (id, kind, owner_user_id, subject_type, subject_id, phase, input) \
         values (gen_random_uuid(), 'agent-hire', $1::uuid, 'agent-hire', $2, '', '{}')",
    )
    .bind(hirer)
    .bind(slug)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "update runs set created_at = now() - ($1 || ' milliseconds')::interval \
         where id in (select id from runs where subject_id = $2 \
                      order by created_at desc limit 1)",
    )
    .bind(ago_millis.to_string())
    .bind(slug)
    .execute(pg)
    .await
    .unwrap();
}

/// A chat conversation owned by `owner`, plus the live turn key pointing at
/// it — the state chat.rs leaves behind mid-turn.
async fn turn(
    pg: &PgPool,
    redis: &mut redis::aio::ConnectionManager,
    model: &str,
    owner: &str,
) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into conversations (user_id, agent_model) values ($1::uuid, $2) \
         returning id::text",
    )
    .bind(owner)
    .bind(model)
    .fetch_one(pg)
    .await
    .unwrap();
    let _: () = redis
        .set_ex(format!("agent-turn:{model}"), &id, 900)
        .await
        .unwrap();
    id
}

async fn clear_turn(redis: &mut redis::aio::ConnectionManager, model: &str) {
    let _: () = redis.del(format!("agent-turn:{model}")).await.unwrap();
}

#[tokio::test]
#[ignore]
async fn a_personal_assistant_outranks_a_live_turn() {
    let pg = pool().await;
    let mut redis = redis().await;
    cleanup(&pg, "attrtest-pa", "pa").await;
    let owner = user(&pg, "owner@pa.attribution-test.invalid").await;
    let model = agent(&pg, "attrtest-pa", Some(&owner)).await;
    // A live turn owned by somebody ELSE — the owner rung must win anyway.
    let other = user(&pg, "chatter@pa.attribution-test.invalid").await;
    turn(&pg, &mut redis, &model, &other).await;
    assert_eq!(
        responsible_user_for(&pg, Some(redis.clone()), &caller(&model))
            .await
            .unwrap(),
        Some(owner)
    );
    clear_turn(&mut redis, &model).await;
    cleanup(&pg, "attrtest-pa", "pa").await;
}

#[tokio::test]
#[ignore]
async fn a_live_turn_outranks_the_hirer() {
    let pg = pool().await;
    let mut redis = redis().await;
    cleanup(&pg, "attrtest-org", "org").await;
    let hirer = user(&pg, "hirer@org.attribution-test.invalid").await;
    let chatter = user(&pg, "chatter@org.attribution-test.invalid").await;
    let model = agent(&pg, "attrtest-org", None).await;
    hire(&pg, "attrtest-org", &hirer, 0).await;
    turn(&pg, &mut redis, &model, &chatter).await;
    assert_eq!(
        responsible_user_for(&pg, Some(redis.clone()), &caller(&model))
            .await
            .unwrap(),
        Some(chatter)
    );
    clear_turn(&mut redis, &model).await;
    cleanup(&pg, "attrtest-org", "org").await;
}

#[tokio::test]
#[ignore]
async fn no_turn_falls_to_the_latest_hirer() {
    let pg = pool().await;
    let mut redis = redis().await;
    cleanup(&pg, "attrtest-rehire", "rehire").await;
    let first = user(&pg, "first@rehire.attribution-test.invalid").await;
    let latest = user(&pg, "latest@rehire.attribution-test.invalid").await;
    let model = agent(&pg, "attrtest-rehire", None).await;
    // Older hire first, fresh hire second — the ladder takes the LATEST.
    hire(&pg, "attrtest-rehire", &first, 3_600_000).await;
    hire(&pg, "attrtest-rehire", &latest, 0).await;
    assert_eq!(
        responsible_user_for(&pg, Some(redis.clone()), &caller(&model))
            .await
            .unwrap(),
        Some(latest)
    );
    cleanup(&pg, "attrtest-rehire", "rehire").await;
}

#[tokio::test]
#[ignore]
async fn a_dead_turn_names_the_hirer_not_a_missing_conversation() {
    let pg = pool().await;
    let mut redis = redis().await;
    cleanup(&pg, "attrtest-deadturn", "deadturn").await;
    let hirer = user(&pg, "hirer@deadturn.attribution-test.invalid").await;
    let model = agent(&pg, "attrtest-deadturn", None).await;
    hire(&pg, "attrtest-deadturn", &hirer, 0).await;
    // A turn key pointing at a conversation that no longer exists (a deleted
    // conversation is the same shape as an expired key to this rung): skip,
    // do not answer None-for-everything.
    let _: () = redis
        .set_ex(
            format!("agent-turn:{model}"),
            "00000000-0000-0000-0000-000000000000",
            900,
        )
        .await
        .unwrap();
    assert_eq!(
        responsible_user_for(&pg, Some(redis.clone()), &caller(&model))
            .await
            .unwrap(),
        Some(hirer)
    );
    clear_turn(&mut redis, &model).await;
    cleanup(&pg, "attrtest-deadturn", "deadturn").await;
}

#[tokio::test]
#[ignore]
async fn a_legacy_caller_gets_nobody() {
    let pg = pool().await;
    let mut redis = redis().await;
    cleanup(&pg, "attrtest-legacy", "legacy").await;
    let owner = user(&pg, "owner@legacy.attribution-test.invalid").await;
    let chatter = user(&pg, "chatter@legacy.attribution-test.invalid").await;
    let model = agent(&pg, "attrtest-legacy", Some(&owner)).await;
    hire(&pg, "attrtest-legacy", &owner, 0).await;
    turn(&pg, &mut redis, &model, &chatter).await;
    // Everything the ladder could grant exists — the legacy key still gets
    // None: identified is not proven.
    assert_eq!(
        responsible_user_for(&pg, Some(redis.clone()), &legacy(&model))
            .await
            .unwrap(),
        None
    );
    clear_turn(&mut redis, &model).await;
    cleanup(&pg, "attrtest-legacy", "legacy").await;
}

#[tokio::test]
#[ignore]
async fn nobody_stands_behind_an_untraceable_agent() {
    let pg = pool().await;
    let mut redis = redis().await;
    cleanup(&pg, "attrtest-orphan", "orphan").await;
    let model = agent(&pg, "attrtest-orphan", None).await;
    // No owner, no turn, no hire run: ownerless is an ordinary answer.
    assert_eq!(
        responsible_user_for(&pg, Some(redis.clone()), &caller(&model))
            .await
            .unwrap(),
        None
    );
    cleanup(&pg, "attrtest-orphan", "orphan").await;
}

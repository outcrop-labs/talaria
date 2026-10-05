// Live-DB proof of the agent-manager gate (cargo test -- --ignored).
//
// What a unit test cannot vouch for here is the whole of it: every answer is
// a Postgres rule — the join against `agent_managers`, the owner-column arm
// that keeps a personal assistant answering to its human, the `not (user_id =
// any(...))` the roster PUT replaces through, and the `/agents` view arm that
// reads the same two tables. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test it agent_managers:: -- --ignored

use crate::support::pg;
use sqlx::postgres::PgPool;
use talaria_agent_managers::{
    add_manager, list_managers, managed_agent_ids, manages_agent, manages_agent_model,
    manages_agent_slug, manages_any_agent, set_managers,
};

const PREFIX: &str = "agent-managers-test";

/// Each test owns its OWN tag, and cleans up only its own rows. The tests in
/// this file run concurrently, so a shared sweep would have one test
/// deleting the agent another was mid-way through asserting against — which
/// is exactly what a foreign-key violation out of `add_manager` turned out to
/// be. The agent_managers rows cascade from both sides.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn user(pg: &PgPool, tag: &str, who: &str, role: &str) -> String {
    let sub = format!("{PREFIX}:{tag}:{who}");
    sqlx::query_scalar::<_, String>(
        "insert into users (sub, email, name, role) values ($1, $1, $2, $3) returning id::text",
    )
    .bind(&sub)
    .bind(who)
    .bind(role)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// A def row only — no key, no version. The gate reads `agent_defs` and
/// `agent_managers`, and nothing else about an agent is in play.
async fn agent(
    pg: &PgPool,
    tag: &str,
    name: &str,
    owner: Option<&str>,
) -> (String, String, String) {
    let slug = format!("{PREFIX}-{tag}-{name}");
    let model = format!("{slug}-model");
    let id = sqlx::query_scalar::<_, String>(
        "insert into agent_defs (slug, department, model, display_name, owner_user_id) \
         values ($1, $1, $2, $3, $4::uuid) returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .bind(name)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, slug, model)
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn only_the_named_managers_and_admins_may_change_an_agent() {
    let pg = pg().await;
    cleanup(&pg, "gate").await;

    let manager = user(&pg, "gate", "manager", "member").await;
    let stranger = user(&pg, "gate", "stranger", "member").await;
    let admin = user(&pg, "gate", "admin", "admin").await;
    let (id, slug, model) = agent(&pg, "gate", "org", None).await;
    add_manager(&pg, &id, &manager, Some(&manager))
        .await
        .unwrap();

    // The named manager, by every key the routes carry.
    assert!(manages_agent(&pg, &manager, "member", &id).await.unwrap());
    assert!(
        manages_agent_model(&pg, &manager, "member", &model)
            .await
            .unwrap()
    );
    assert!(
        manages_agent_slug(&pg, &manager, "member", &slug)
            .await
            .unwrap()
    );

    // A member who was not named — the case the whole change exists for. A
    // permission is not asked anywhere in this answer: `agents.manage` runs
    // the fleet, it does not confer the right to rewrite someone's agent.
    assert!(!manages_agent(&pg, &stranger, "member", &id).await.unwrap());
    assert!(
        !manages_agent_model(&pg, &stranger, "member", &model)
            .await
            .unwrap()
    );
    assert!(
        !manages_agent_slug(&pg, &stranger, "member", &slug)
            .await
            .unwrap()
    );

    // An admin manages every agent, named or not — an agent must not leave
    // with the person who owned it.
    assert!(manages_agent(&pg, &admin, "admin", &id).await.unwrap());
    assert!(manages_any_agent(&pg, &admin, "admin").await.unwrap());

    // A path segment that is not a uuid is "no", not a 500 from the cast.
    assert!(
        !manages_agent(&pg, &manager, "member", "undefined")
            .await
            .unwrap()
    );
    assert!(!manages_agent(&pg, &manager, "member", "").await.unwrap());

    cleanup(&pg, "gate").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_personal_assistants_human_manages_it_without_a_row() {
    let pg = pg().await;
    cleanup(&pg, "assistant").await;

    let owner = user(&pg, "assistant", "assistant-owner", "member").await;
    let other = user(&pg, "assistant", "assistant-other", "member").await;
    // owner_user_id only — the legacy arm, for an assistant minted by a path
    // that predates (or forgets) the manager row.
    let (id, slug, model) = agent(&pg, "assistant", "assistant", Some(&owner)).await;
    assert_eq!(list_managers(&pg, &id).await.unwrap().len(), 0);

    assert!(manages_agent(&pg, &owner, "member", &id).await.unwrap());
    assert!(
        manages_agent_slug(&pg, &owner, "member", &slug)
            .await
            .unwrap()
    );
    assert!(
        manages_agent_model(&pg, &owner, "member", &model)
            .await
            .unwrap()
    );
    assert!(manages_any_agent(&pg, &owner, "member").await.unwrap());
    assert!(!manages_agent(&pg, &other, "member", &id).await.unwrap());
    assert!(!manages_any_agent(&pg, &other, "member").await.unwrap());

    cleanup(&pg, "assistant").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_roster_put_replaces_the_set_and_the_handover_takes_effect() {
    let pg = pg().await;
    cleanup(&pg, "handover").await;

    let first = user(&pg, "handover", "handover-first", "member").await;
    let second = user(&pg, "handover", "handover-second", "member").await;
    let (id, _slug, _model) = agent(&pg, "handover", "handover", None).await;
    add_manager(&pg, &id, &first, Some(&first)).await.unwrap();
    // Naming the same person twice is not an error — the hire path and the
    // roster PUT both insert, and a duplicate must not fail a write.
    add_manager(&pg, &id, &first, Some(&first)).await.unwrap();
    assert_eq!(list_managers(&pg, &id).await.unwrap().len(), 1);

    // Hand it over: both, then only the second. The replace is what the PUT
    // writes, so the dropped manager must actually stop managing it.
    set_managers(&pg, &id, &[first.clone(), second.clone()], Some(&first))
        .await
        .unwrap();
    assert_eq!(list_managers(&pg, &id).await.unwrap().len(), 2);
    assert!(manages_agent(&pg, &second, "member", &id).await.unwrap());

    set_managers(&pg, &id, std::slice::from_ref(&second), Some(&first))
        .await
        .unwrap();
    let after = list_managers(&pg, &id).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].user_id, second);
    assert!(!manages_agent(&pg, &first, "member", &id).await.unwrap());
    assert!(!manages_any_agent(&pg, &first, "member").await.unwrap());

    cleanup(&pg, "handover").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_roster_read_narrows_to_the_agents_a_person_manages() {
    let pg = pg().await;
    cleanup(&pg, "roster").await;

    let person = user(&pg, "roster", "roster", "member").await;
    let (mine, _, _) = agent(&pg, "roster", "roster-mine", None).await;
    let (owned, _, _) = agent(&pg, "roster", "roster-owned", Some(&person)).await;
    let (theirs, _, _) = agent(&pg, "roster", "roster-theirs", None).await;
    add_manager(&pg, &mine, &person, Some(&person))
        .await
        .unwrap();

    let ids = managed_agent_ids(&pg, &person).await.unwrap();
    assert!(ids.contains(&mine));
    assert!(ids.contains(&owned), "the owner arm counts as managing");
    assert!(!ids.contains(&theirs));

    cleanup(&pg, "roster").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn managing_an_agent_opens_the_agents_view_by_itself() {
    let pg = pg().await;
    cleanup(&pg, "view").await;

    // No allowed_manage_views grant anywhere in this test: /agents is a
    // manage view, denied to members by default, and being named a manager
    // is what opens it.
    let manager = user(&pg, "view", "view-manager", "member").await;
    let stranger = user(&pg, "view", "view-stranger", "member").await;
    let (id, _, _) = agent(&pg, "view", "view", None).await;

    let denied = talaria_users::denied_views(&pg, &stranger, "member")
        .await
        .unwrap();
    assert!(denied.iter().any(|v| v == "/agents"));

    add_manager(&pg, &id, &manager, Some(&manager))
        .await
        .unwrap();
    let denied = talaria_users::denied_views(&pg, &manager, "member")
        .await
        .unwrap();
    assert!(
        !denied.iter().any(|v| v == "/agents"),
        "a manager reaches the surface their agent lives on"
    );
    // The other manage views are untouched — this is one view, not a
    // back door into Teams, Models or MCP.
    assert!(denied.iter().any(|v| v == "/models"));
    assert!(denied.iter().any(|v| v == "/mcp"));
    assert!(denied.iter().any(|v| v == "/teams"));

    cleanup(&pg, "view").await;
}

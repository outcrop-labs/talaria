// Live-DB proof that the Developer Agent switch is the one and only grant of
// the Workbench MCP server (cargo test -- --ignored). The guarantee lives in
// two SQL paths, the render's roster (`servers_for_agent`) and the gateway's
// per-call check (`effective_mcp_for`), so only a real database can vouch for
// it. The case that broke in production is the first assertion: an agent set
// up everywhere EXCEPT one out-of-the-way grant had no tools; now a registry
// row neither grants nor withholds them. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test it developer_agent_live:: -- --ignored

use crate::support::pg;
use sqlx::postgres::PgPool;
use talaria_api::agent_auth::AgentSubject;
use talaria_api::secretbox::SecretBox;
use talaria_mcp::registry::{
    WORKBENCH_SERVER, effective_mcp_for, ensure_builtin_mcp, servers_for_agent,
};
use talaria_workbench::set_developer;

async fn cleanup(pg: &PgPool, model: &str) {
    sqlx::query("delete from mcp_server_agents where agent_model = $1")
        .bind(model)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where model = $1")
        .bind(model)
        .execute(pg)
        .await
        .unwrap();
}

async fn has_workbench(pg: &PgPool, model: &str) -> (bool, bool) {
    let rostered = servers_for_agent(pg, model)
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == WORKBENCH_SERVER);
    let callable = effective_mcp_for(
        pg,
        &SecretBox::default(),
        &AgentSubject::Model(model.to_string()),
        WORKBENCH_SERVER,
    )
    .await
    .unwrap()
    .is_some();
    (rostered, callable)
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_developer_switch_alone_grants_the_workbench() {
    let pg = pg().await;
    ensure_builtin_mcp(&pg).await.unwrap();
    let slug = "devagentlive";
    let model = "devagentlive-engineering";
    cleanup(&pg, model).await;
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'engineering', $2, 'Developer Agent Live') returning id::text",
    )
    .bind(slug)
    .bind(model)
    .fetch_one(&pg)
    .await
    .unwrap();

    // A leftover registry assignment (what the old settings wrote) does
    // not grant the Workbench to an agent with the switch off.
    sqlx::query(
        "insert into mcp_server_agents (server_id, agent_model) \
         select id, $1 from mcp_servers where name = $2",
    )
    .bind(model)
    .bind(WORKBENCH_SERVER)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(
        has_workbench(&pg, model).await,
        (false, false),
        "switch off: neither rendered nor callable, whatever the registry rows say"
    );

    // On: rendered into the agent's config AND callable at the gateway.
    assert!(
        set_developer(&pg, &id, true).await.unwrap(),
        "off → on changes"
    );
    assert!(
        !set_developer(&pg, &id, true).await.unwrap(),
        "on → on is not a change, so the route does not roll the agent again"
    );
    assert_eq!(has_workbench(&pg, model).await, (true, true));

    // Off again: gone from both, even with the registry row still there.
    assert!(set_developer(&pg, &id, false).await.unwrap());
    assert_eq!(has_workbench(&pg, model).await, (false, false));

    cleanup(&pg, model).await;
}

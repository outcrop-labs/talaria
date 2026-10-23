// Live-DB proof that an app's MCP server keeps its grants when the app stops
// serving, and that the app sync never overrides an admin's own switch
// (talaria-apps `sync_app_mcp_servers`). The guarantees live in SQL: the
// upsert that leaves `enabled` alone on an existing row, the `app_off` flag
// that replaced a delete which cascaded every agent, team and user grant, and
// the render's `app_off is null`. Only a real database can vouch for them.
// House rule: #[ignore]d; CI's api-integration job runs it against the
// migrated scratch database.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it app_mcp_sync_live:: -- --ignored

use crate::support::{fabricate_user, pg, sweep_user_rows};
use sqlx::postgres::PgPool;
use std::collections::BTreeSet;
use talaria_api::apps::{AppFacts, app_facts, remove_app_mcp_server, sync_app_mcp_servers_over};
use talaria_api::secretbox::SecretBox;
use talaria_mcp::registry::{ServerPatch, servers_for_agent, update_mcp_server};

const PREFIX: &str = "app-mcp-sync-live";
const SLUG: &str = "livesync";
const SERVER: &str = "app-livesync";
const DIRECT: &str = "livesync-direct-agent";
const BY_TEAM: &str = "livesync-team-agent";
const TEAM: &str = "app-mcp-sync-live team";

/// One sync over the disk's apps plus this test's app, which is not on disk.
async fn sync(pg: &PgPool, enabled: bool, failed: Option<&str>) -> Vec<String> {
    let fact = AppFacts {
        slug: SLUG.into(),
        name: "Live sync".into(),
        description: String::new(),
        mcp: true,
        enabled,
        failed: failed.map(String::from),
    };
    sync_app_mcp_servers_over(pg, &SecretBox::default(), move |on: &BTreeSet<String>| {
        let mut facts = app_facts(on);
        facts.push(fact);
        facts
    })
    .await
    .expect("sync")
}

/// The row's admin switch and app-off reason.
async fn row(pg: &PgPool) -> Option<(String, bool, Option<String>)> {
    sqlx::query_as("select id::text, enabled, app_off from mcp_servers where name = $1")
        .bind(SERVER)
        .fetch_optional(pg)
        .await
        .unwrap()
}

/// (agent assignments, user grants, team grants) on one server id.
async fn grants(pg: &PgPool, id: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "select (select count(*) from mcp_server_agents where server_id::text = $1), \
                (select count(*) from mcp_user_access where server_id::text = $1), \
                (select count(*) from mcp_team_access where server_id::text = $1)",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// Whether the agent's rendered config lists the server.
async fn carried(pg: &PgPool, model: &str) -> bool {
    servers_for_agent(pg, model)
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == SERVER)
}

/// The admin's switch, through the registry's own update (the door
/// `PUT /api/mcp/servers/{id}` takes), which admits `enabled` on an app row.
async fn set_enabled(pg: &PgPool, id: &str, on: bool) {
    update_mcp_server(
        pg,
        &SecretBox::default(),
        id,
        &ServerPatch {
            enabled: Some(on),
            ..Default::default()
        },
    )
    .await
    .expect("an admin may switch an app server");
}

async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from mcp_servers where name = $1")
        .bind(SERVER)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from teams where name = $1")
        .bind(TEAM)
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, PREFIX, "user").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_app_server_keeps_its_grants_and_the_admins_switch() {
    let pg = pg().await;
    cleanup(&pg).await;

    // The app serves: its row is created, on, with no reason.
    assert!(sync(&pg, true, None).await.contains(&SERVER.to_string()));
    let (id, enabled, off) = row(&pg).await.expect("the row exists");
    assert!(enabled);
    assert_eq!(off, None);

    // Grants of every kind: an agent by assignment, an agent through a team,
    // and a person.
    sqlx::query("insert into mcp_server_agents (server_id, agent_model) values ($1::uuid, $2)")
        .bind(&id)
        .bind(DIRECT)
        .execute(&pg)
        .await
        .unwrap();
    let user = fabricate_user(&pg, PREFIX, "user").await;
    sqlx::query("insert into mcp_user_access (server_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&id)
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
    let team: String =
        sqlx::query_scalar("insert into teams (name) values ($1) returning id::text")
            .bind(TEAM)
            .fetch_one(&pg)
            .await
            .unwrap();
    sqlx::query("insert into team_agents (team_id, agent_model) values ($1::uuid, $2)")
        .bind(&team)
        .bind(BY_TEAM)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query("insert into mcp_team_access (server_id, team_id) values ($1::uuid, $2::uuid)")
        .bind(&id)
        .bind(&team)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(grants(&pg, &id).await, (1, 1, 1));
    assert!(carried(&pg, DIRECT).await);
    assert!(carried(&pg, BY_TEAM).await);

    // An app server has no address or credentials to change.
    let repoint = update_mcp_server(
        &pg,
        &SecretBox::default(),
        &id,
        &ServerPatch {
            url: Some("https://elsewhere.example/mcp".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(repoint.is_err(), "{repoint:?}");

    // An admin turns the server off. Resyncs (every boot runs one) leave it off.
    set_enabled(&pg, &id, false).await;
    sync(&pg, true, None).await;
    sync(&pg, true, None).await;
    assert_eq!(row(&pg).await, Some((id.clone(), false, None)));
    assert!(!carried(&pg, DIRECT).await);
    set_enabled(&pg, &id, true).await;

    // The build breaks and the app is turned off: the row turns off with the
    // reason and leaves every agent, and every grant is still there.
    let serving = sync(&pg, false, Some("mcp.ts: SyntaxError")).await;
    assert!(!serving.contains(&SERVER.to_string()));
    assert_eq!(
        row(&pg).await,
        Some((
            id.clone(),
            true,
            Some("the app failed to build or load: mcp.ts: SyntaxError".into())
        ))
    );
    assert_eq!(grants(&pg, &id).await, (1, 1, 1));
    assert!(!carried(&pg, DIRECT).await);
    assert!(!carried(&pg, BY_TEAM).await);

    // An admin who turns the server off while its app is off still has it
    // off when the app comes back.
    set_enabled(&pg, &id, false).await;
    assert!(sync(&pg, true, None).await.contains(&SERVER.to_string()));
    assert_eq!(row(&pg).await, Some((id.clone(), false, None)));
    assert!(!carried(&pg, DIRECT).await);

    // Turned back on: the same row serves the same grants.
    set_enabled(&pg, &id, true).await;
    assert_eq!(grants(&pg, &id).await, (1, 1, 1));
    assert!(carried(&pg, DIRECT).await);
    assert!(carried(&pg, BY_TEAM).await);

    // An app gone from disk without an uninstall: off, never deleted.
    sync_app_mcp_servers_over(&pg, &SecretBox::default(), app_facts)
        .await
        .expect("sync");
    assert_eq!(
        row(&pg).await,
        Some((id.clone(), true, Some("the app is not installed".into())))
    );
    assert_eq!(grants(&pg, &id).await, (1, 1, 1));

    // Uninstall is the one delete, and the grants go with the row.
    remove_app_mcp_server(&pg, &SecretBox::default(), SLUG)
        .await
        .expect("remove");
    assert_eq!(row(&pg).await, None);
    assert_eq!(grants(&pg, &id).await, (0, 0, 0));

    cleanup(&pg).await;
}

// Live-DB proof of the stored-config seal (talaria-fleet-seal): a def whose
// history holds a credential as written — v1 one value, v2 another — comes
// out with a handle in EVERY version, one per-agent secret holding the
// current value, and a second run that finds nothing to do. The dry run
// writes nothing. House rule: #[ignore]d; CI's api-integration job runs it
// against a scratch database.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it seal_configs_live:: -- --ignored

use crate::support::pg;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use talaria_api::secretbox::SecretBox;
use talaria_fleet_seal::{SealRequest, seal_stored_configs};

const SLUG: &str = "sealconfigslive";

async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from agent_defs where slug = $1")
        .bind(SLUG)
        .execute(pg)
        .await
        .unwrap();
}

async fn literal_rows(pg: &PgPool, id: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from agent_versions where agent_id = $1::uuid \
         and (config::text like '%tg-live-A%' or config::text like '%tg-live-B%')",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn every_version_is_sealed_with_the_current_value_and_a_rerun_is_a_no_op() {
    let pg = pg().await;
    cleanup(&pg).await;
    // A box with keys in hand: the seal writes ciphertext, the test opens it.
    let sb = SecretBox::from_parts([7u8; 32], HashMap::from([(1, [9u8; 32])]), Some(1));
    // Unmanaged: nothing to render or roll, only the stored history.
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, managed, enabled) \
         values ($1, $1, $1, 'Seal Configs Live', false, false) returning id::text",
    )
    .bind(SLUG)
    .fetch_one(&pg)
    .await
    .unwrap();
    for (version, token) in [(1, "tg-live-A"), (2, "tg-live-B")] {
        let config = json!({
            "main": { "endpoint": "e", "model": "m" },
            "raw": { "platforms": { "telegram": { "enabled": true, "token": token } } },
        });
        sqlx::query(
            "insert into agent_versions (agent_id, version, soul, config, note, created_by) \
             values ($1::uuid, $2, 'soul', $3, 'pre-579', 'test')",
        )
        .bind(&id)
        .bind(version)
        .bind(&config)
        .execute(&pg)
        .await
        .unwrap();
    }
    sqlx::query("update agent_defs set current_version = 2 where id = $1::uuid")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();

    let req = |dry_run| SealRequest {
        dry_run,
        agent_id: Some(id.as_str()),
        drop_undeliverable: false,
        actor: "seal-configs-live",
    };

    // The dry run reports and writes nothing.
    let dry = seal_stored_configs(&pg, &sb, &req(true)).await.unwrap();
    let mine = &dry.agents[0];
    assert_eq!(mine.status, "planned", "{mine:?}");
    assert_eq!(mine.versions, vec![1, 2]);
    assert_eq!(mine.sealed, vec!["CONFIG_PLATFORMS_TELEGRAM_TOKEN"]);
    assert_eq!(mine.changed, vec!["platforms.telegram.token (v1)"]);
    assert!(!mine.roll, "an unmanaged agent never rolls");
    assert_eq!(
        literal_rows(&pg, &id).await,
        2,
        "the dry run rewrote nothing"
    );
    let secrets: i64 =
        sqlx::query_scalar("select count(*) from agent_secrets where agent_id = $1::uuid")
            .bind(&id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(secrets, 0, "the dry run sealed nothing");

    // Applied: every version names the secret, which holds the CURRENT value.
    let applied = seal_stored_configs(&pg, &sb, &req(false)).await.unwrap();
    assert_eq!(
        applied.agents[0].status, "sealed",
        "{:?}",
        applied.agents[0]
    );
    assert_eq!(literal_rows(&pg, &id).await, 0);
    let handles: Vec<String> = sqlx::query_scalar(
        "select config->'raw'->'platforms'->'telegram'->>'token' from agent_versions \
         where agent_id = $1::uuid order by version",
    )
    .bind(&id)
    .fetch_all(&pg)
    .await
    .unwrap();
    assert_eq!(
        handles,
        vec!["${CONFIG_PLATFORMS_TELEGRAM_TOKEN}"; 2],
        "both rows, rewritten in place"
    );
    let enc: String = sqlx::query_scalar(
        "select value_enc from agent_secrets where agent_id = $1::uuid \
         and name = 'CONFIG_PLATFORMS_TELEGRAM_TOKEN'",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(sb.open(&enc).unwrap(), "tg-live-B");
    let versions: i64 =
        sqlx::query_scalar("select count(*) from agent_versions where agent_id = $1::uuid")
            .bind(&id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(versions, 2, "a redaction, not a new version");

    // A second run finds nothing to do.
    let again = seal_stored_configs(&pg, &sb, &req(false)).await.unwrap();
    assert_eq!(again.agents[0].status, "clean", "{:?}", again.agents[0]);
    assert!(again.agents[0].versions.is_empty());

    cleanup(&pg).await;
}

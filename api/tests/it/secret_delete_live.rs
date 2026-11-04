// Live-DB + live-Redis proof of the guard on a sealed credential's delete
// (DELETE /api/fleet/agents/{id}/secrets) and of the revert that leaves a
// setting out when its sealed credential is gone (POST
// /api/fleet/defs/{id}/versions). Both read the agent's stored config and its
// secret rows, and both write a version, so only the real router against a
// real database proves them:
//
// - the list says where each CONFIG_* secret is used;
// - deleting one the current config names is a 409 carrying `usedAt`, by
//   body or by the old `?name=` spelling, and nothing is deleted;
// - a `removeSetting` that is not a boolean is a 400;
// - an unreferenced CONFIG_* secret deletes as before;
// - `removeSetting: true` writes a new version without the setting, deletes
//   the row and audits both;
// - a revert to a version that names a secret the agent no longer holds
//   leaves that setting out (and writes nothing when that equals the latest).
//
// The def is unmanaged, so nothing here renders or touches a container.
// House rule: #[ignore]d; CI's api-integration job runs it.
//
//   source ui/.env && cargo test -p talaria-api --test it secret_delete_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const PREFIX: &str = "secret-delete-live";
const PASSWORD: &str = "CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD";
const TOKEN: &str = "CONFIG_PLATFORMS_TELEGRAM_TOKEN";
const UNUSED: &str = "CONFIG_PLATFORMS_SLACK_BOT_TOKEN";

async fn cleanup(pg: &PgPool) {
    sqlx::query(
        "delete from audit_log where target_type = 'agent' and target_id in \
         (select id::text from agent_defs where slug like $1)",
    )
    .bind(format!("{PREFIX}-%"))
    .execute(pg)
    .await
    .unwrap();
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// An admin with a session minted the way the auth routes mint one: the
/// routes ask `require_agent_manager`, which an admin passes for any agent.
async fn minted_admin(state: &AppState) -> String {
    let sub = format!("{PREFIX}:{}", uuid::Uuid::new_v4());
    let (id,): (String,) =
        sqlx::query_as("insert into users (sub, role) values ($1, 'admin') returning id::text")
            .bind(&sub)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let user = SessionUser {
        id,
        sub,
        email: None,
        name: None,
        picture: None,
        role: "admin".into(),
        provider: "password".into(),
    };
    let sid = create_session(state, &user)
        .await
        .expect("session mints — needs REDIS_URL");
    format!("talaria_session={sid}")
}

/// An unmanaged def whose v1 names two sealed credentials, holding those two
/// and a third nothing names. The rows' ciphertext is never opened here.
async fn agent(pg: &PgPool) -> String {
    let slug = format!("{PREFIX}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, managed, enabled) \
         values ($1, $1, $1, 'Secret Delete Live', false, false) returning id::text",
    )
    .bind(&slug)
    .fetch_one(pg)
    .await
    .unwrap();
    let config = json!({
        "main": { "endpoint": "e", "model": "m" },
        "raw": {
            "dashboard": { "basic_auth": { "username": "ops", "password": format!("${{{PASSWORD}}}") } },
            "platforms": { "telegram": { "enabled": true, "token": format!("${{{TOKEN}}}") } },
        },
    });
    sqlx::query(
        "insert into agent_versions (agent_id, version, soul, config, note, created_by) \
         values ($1::uuid, 1, 'soul', $2, 'v1', 'test')",
    )
    .bind(&id)
    .bind(&config)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query("update agent_defs set current_version = 1 where id = $1::uuid")
        .bind(&id)
        .execute(pg)
        .await
        .unwrap();
    for name in [PASSWORD, TOKEN, UNUSED] {
        sqlx::query(
            "insert into agent_secrets (agent_id, name, value_enc, updated_by) \
             values ($1::uuid, $2, 'never-opened', 'test')",
        )
        .bind(&id)
        .bind(name)
        .execute(pg)
        .await
        .unwrap();
    }
    id
}

async fn held(pg: &PgPool, id: &str) -> Vec<String> {
    sqlx::query_scalar("select name from agent_secrets where agent_id = $1::uuid order by name")
        .bind(id)
        .fetch_all(pg)
        .await
        .unwrap()
}

async fn version(pg: &PgPool, id: &str, v: i32) -> (Value, Option<String>) {
    sqlx::query_as(
        "select config, note from agent_versions where agent_id = $1::uuid and version = $2",
    )
    .bind(id)
    .bind(v)
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn latest(pg: &PgPool, id: &str) -> i32 {
    sqlx::query_scalar("select current_version from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_one(pg)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_sealed_credential_in_use_goes_only_with_its_setting() {
    let state = live_app_state().await;
    cleanup(&state.pg).await;
    let cookie = minted_admin(&state).await;
    let id = agent(&state.pg).await;
    let secrets = format!("/api/fleet/agents/{id}/secrets");

    // The list says where each CONFIG_* secret is used.
    let (status, body) = call_json(&state, "GET", &secrets, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let used = |name: &str| {
        body["secrets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .map(|s| s["usedAt"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(used(PASSWORD), json!(["dashboard.basic_auth.password"]));
    assert_eq!(used(TOKEN), json!(["platforms.telegram.token"]));
    assert_eq!(used(UNUSED), json!([]));

    // In use: a 409 naming where, by body and by the old query spelling.
    let (status, body) = call_json(
        &state,
        "DELETE",
        &secrets,
        Some(&cookie),
        Some(json!({ "name": PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["usedAt"], json!(["dashboard.basic_auth.password"]));
    assert!(body["error"].as_str().unwrap().contains(PASSWORD), "{body}");
    let (status, body) = call_json(
        &state,
        "DELETE",
        &format!("{secrets}?name={PASSWORD}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // Not a boolean: a 400, not a quiet "no".
    let (status, body) = call_json(
        &state,
        "DELETE",
        &secrets,
        Some(&cookie),
        Some(json!({ "name": PASSWORD, "removeSetting": "yes" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(held(&state.pg, &id).await.len(), 3, "nothing was deleted");
    assert_eq!(latest(&state.pg, &id).await, 1, "and no version written");

    // Unreferenced: deletes as before, with no version.
    let (status, body) = call_json(
        &state,
        "DELETE",
        &secrets,
        Some(&cookie),
        Some(json!({ "name": UNUSED })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("version").is_none(), "{body}");
    assert_eq!(held(&state.pg, &id).await, vec![PASSWORD, TOKEN]);
    assert_eq!(latest(&state.pg, &id).await, 1);

    // removeSetting: a new version without the setting, then the row goes.
    let (status, body) = call_json(
        &state,
        "DELETE",
        &secrets,
        Some(&cookie),
        Some(json!({ "name": PASSWORD, "removeSetting": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], json!(2));
    assert_eq!(body["removed"], json!(["dashboard.basic_auth.password"]));
    assert_eq!(held(&state.pg, &id).await, vec![TOKEN]);
    assert_eq!(latest(&state.pg, &id).await, 2);
    let (v2, note) = version(&state.pg, &id, 2).await;
    assert_eq!(
        v2["raw"]["dashboard"]["basic_auth"],
        json!({ "username": "ops" })
    );
    assert_eq!(
        v2["raw"]["platforms"]["telegram"]["token"],
        json!(format!("${{{TOKEN}}}")),
        "every other handle stays"
    );
    assert!(
        note.as_deref()
            .is_some_and(|n| n.contains("dashboard.basic_auth.password")),
        "{note:?}"
    );
    let edit: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where target_type = 'agent' and target_id = $1 \
         and action = 'agent.edit' and after->>'version' = '2'",
    )
    .bind(&id)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(edit, 1, "the new version is audited as an edit");
    // The delete's own row is written off the caller's clock.
    let mut deleted = 0i64;
    for _ in 0..50 {
        deleted = sqlx::query_scalar(
            "select count(*) from audit_log where target_type = 'agent' and target_id = $1 \
             and action = 'agent.secret_delete' and after->>'name' = $2 \
             and after->'removedSetting' = '[\"dashboard.basic_auth.password\"]'::jsonb",
        )
        .bind(&id)
        .bind(PASSWORD)
        .fetch_one(&state.pg)
        .await
        .unwrap();
        if deleted == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(deleted, 1, "the delete is audited with the setting it took");

    // A revert to v1 leaves the password out (its secret is gone), which is
    // exactly v2: nothing new is written.
    let versions = format!("/api/fleet/defs/{id}/versions");
    let (status, body) = call_json(
        &state,
        "POST",
        &versions,
        Some(&cookie),
        Some(json!({ "revertTo": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], json!(false), "{body}");
    assert_eq!(body["version"], json!(2));

    // A row removed outside the route (a hand edit, a secret-health clear):
    // the next revert leaves that setting out too, and says where.
    sqlx::query("delete from agent_secrets where agent_id = $1::uuid and name = $2")
        .bind(&id)
        .bind(TOKEN)
        .execute(&state.pg)
        .await
        .unwrap();
    let (status, body) = call_json(
        &state,
        "POST",
        &versions,
        Some(&cookie),
        Some(json!({ "revertTo": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], json!(true), "{body}");
    assert_eq!(body["version"], json!(3));
    let (v3, note) = version(&state.pg, &id, 3).await;
    assert_eq!(
        v3["raw"]["dashboard"]["basic_auth"],
        json!({ "username": "ops" })
    );
    assert_eq!(
        v3["raw"]["platforms"]["telegram"],
        json!({ "enabled": true })
    );
    let note = note.unwrap_or_default();
    assert!(
        note.contains("dashboard.basic_auth.password") && note.contains("platforms.telegram.token"),
        "{note}"
    );

    cleanup(&state.pg).await;
}

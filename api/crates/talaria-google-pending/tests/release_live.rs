// Live proof that an approval whose token vend FAILS goes back to `pending`
// (cargo test -- --ignored). The release is an update Postgres commits, so no
// unit test can vouch for it. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-google-pending --test release_live -- --ignored
//
//   google refused    the client record names an OAuth client Google does not
//                     know; the refresh reaches Google's real token endpoint
//                     with a garbage refresh token and NO real credential, and
//                     Google answers `invalid_client`. The decision answers
//                     not_connected quoting Google, the row is pending again
//                     with that sentence as `result.error`, and the stored
//                     grant is untouched (a refused client never clears one).
//                     Needs the network.
//   local fault       no client configured anywhere: nothing reaches Google,
//                     the decision is still the hard failure it was, and the
//                     row is pending again instead of stranded `executing`.
//
// Each test builds its OWN schema with the three tables the path touches and
// points the pool's search_path at it alone.

use std::collections::HashMap;
use std::str::FromStr;

use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use talaria_secretbox::SecretBox;

const OWNER: &str = "00000000-0000-4000-8000-0000000000a1";

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://talaria:talaria@127.0.0.1:5432/talaria".into())
}

fn test_box() -> SecretBox {
    let kek = talaria_secretbox::derive_kek("release-live-test");
    let dek = talaria_secretbox::new_dek().expect("entropy");
    SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
}

/// A throwaway schema: the pending queue, one owner connection whose access
/// cache has expired (so approving must refresh), one pending draft.
async fn scratch(tag: &str, sb: &SecretBox) -> (PgPool, PgPool, String, String) {
    let admin = PgPool::connect(&database_url())
        .await
        .expect("dev database");
    let schema = format!("google_release_live_{tag}_{}", std::process::id());
    let prefix = format!("google_release_live_{tag}_");
    let stale: Vec<String> =
        sqlx::query_scalar("select nspname::text from pg_namespace where starts_with(nspname, $1)")
            .bind(&prefix)
            .fetch_all(&admin)
            .await
            .expect("list scratch schemas");
    for old in stale {
        // AssertSqlSafe: names this file's own tag made, read back from pg.
        let sql = format!("drop schema if exists {old} cascade");
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&admin)
            .await
            .expect("sweep a stale scratch schema");
    }
    for sql in [
        format!("create schema {schema}"),
        format!(
            "create table {schema}.google_pending_actions (id uuid primary key default gen_random_uuid(), \
             kind text not null, payload jsonb not null, summary text, agent_model text, owner_user_id uuid, \
             status text not null default 'pending', result jsonb, created_at timestamptz not null default now(), \
             decided_at timestamptz, decided_by uuid, is_org boolean not null default false, \
             principal_kind text not null default 'org')"
        ),
        format!(
            "create table {schema}.google_connections (user_id uuid primary key, google_sub text not null, \
             email text, scope text not null default '', refresh_token_enc text, access_token_enc text, \
             access_expires_at timestamptz, created_at timestamptz not null default now(), \
             updated_at timestamptz not null default now())"
        ),
        format!(
            "create table {schema}.app_settings (key text primary key, value jsonb not null, \
             updated_at timestamptz not null default now())"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&admin)
            .await
            .expect(&sql);
    }
    let opts = PgConnectOptions::from_str(&database_url())
        .expect("DATABASE_URL parses")
        .options([("search_path", schema.as_str())]);
    let pg = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .expect("scratch pool");
    sqlx::query(
        "insert into google_connections (user_id, google_sub, refresh_token_enc, access_token_enc, access_expires_at) \
         values ($1::uuid, 'sub-release-live', $2, $3, now() - interval '1 hour')",
    )
    .bind(OWNER)
    .bind(sb.seal("1//release-live-not-a-refresh-token").unwrap())
    .bind(sb.seal("expired-access").unwrap())
    .execute(&pg)
    .await
    .expect("connection row");
    let id: String = sqlx::query_scalar(
        "insert into google_pending_actions (kind, payload, summary, owner_user_id, is_org, principal_kind) \
         values ('gmail_send', $1, 'release-live', $2::uuid, false, 'owner') returning id::text",
    )
    .bind(json!({ "to": "nobody@example.invalid", "subject": "release-live", "body": "never sent" }))
    .bind(OWNER)
    .fetch_one(&pg)
    .await
    .expect("pending row");
    (pg, admin, schema, id)
}

/// The file's own invariant, checked last in every test: no row is left
/// claimed. Then close the pool (its sessions hold the schema) and drop it.
async fn teardown(pg: PgPool, admin: &PgPool, schema: &str) {
    let claimed: i64 = sqlx::query_scalar(
        "select count(*)::int8 from google_pending_actions where status = 'executing'",
    )
    .fetch_one(&pg)
    .await
    .expect("count claimed rows");
    assert_eq!(claimed, 0, "no row is left stranded `executing`");
    pg.close().await;
    // AssertSqlSafe: the schema name is this file's own tag and a pid.
    let sql = format!("drop schema if exists {schema} cascade");
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .execute(admin)
        .await
        .expect("drop the scratch schema");
}

async fn row(pg: &PgPool, id: &str) -> (String, Option<String>, Option<String>) {
    sqlx::query_as(
        "select status, result->>'error', decided_by::text from google_pending_actions where id = $1::uuid",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .expect("row")
}

#[tokio::test]
#[ignore = "needs a live Postgres (DATABASE_URL) and Google's token endpoint"]
async fn a_refused_client_releases_the_row_and_quotes_google() {
    let sb = test_box();
    let (pg, admin, schema, id) = scratch("refused", &sb).await;
    sqlx::query("insert into app_settings (key, value) values ('google_oauth_client', $1)")
        .bind(json!({
            "clientId": "talaria-release-live-not-a-client.apps.googleusercontent.com",
            "clientSecretEnc": sb.seal("not-a-secret").unwrap(),
        }))
        .execute(&pg)
        .await
        .expect("client record");

    let outcome = talaria_google_pending::decide_action(
        &pg,
        &sb,
        &id,
        OWNER,
        false,
        "approve",
        talaria_agent_auth::now_ms(),
    )
    .await
    .expect("a refusal Google explained is an answer, not a hard failure")
    .expect("the row exists");
    assert_eq!(outcome.status, "not_connected");
    let message = outcome.message.expect("a sentence");
    assert!(
        message.starts_with(
            "Google refused this server's OAuth client: «invalid_client: The OAuth client was not found.»"
        ),
        "Google's own words: {message}"
    );

    let (status, error, decided_by) = row(&pg, &id).await;
    assert_eq!(status, "pending", "released, not stranded executing");
    assert_eq!(
        error.as_deref(),
        Some(message.as_str()),
        "the reason rides the row"
    );
    assert_eq!(decided_by, None);
    let listed = talaria_google_pending::list_pending(&pg, OWNER, false)
        .await
        .expect("list");
    assert_eq!(
        talaria_google_pending::pending_wire(&listed[0])["lastError"],
        json!(message)
    );
    let kept: Option<String> = sqlx::query_scalar(
        "select refresh_token_enc from google_connections where user_id = $1::uuid",
    )
    .bind(OWNER)
    .fetch_one(&pg)
    .await
    .expect("connection");
    assert!(kept.is_some(), "a refused client never clears the grant");

    teardown(pg, &admin, &schema).await;
}

#[tokio::test]
#[ignore = "needs a live Postgres (DATABASE_URL)"]
async fn a_token_fault_with_no_google_answer_still_releases_the_row() {
    // SAFETY: the only env this binary reads; the other test's DB record
    // wins over env, so removing it cannot change that test.
    unsafe {
        std::env::remove_var("AUTH_GOOGLE_CLIENT_ID");
        std::env::remove_var("AUTH_GOOGLE_CLIENT_SECRET");
    }
    let sb = test_box();
    let (pg, admin, schema, id) = scratch("local", &sb).await;

    let failed = talaria_google_pending::decide_action(
        &pg,
        &sb,
        &id,
        OWNER,
        false,
        "approve",
        talaria_agent_auth::now_ms(),
    )
    .await;
    assert_eq!(
        failed.err().as_deref(),
        Some("no google client configured"),
        "still the hard failure it was"
    );
    let (status, error, _) = row(&pg, &id).await;
    assert_eq!(status, "pending", "released, not stranded executing");
    assert_eq!(
        error.as_deref(),
        Some(
            "Nothing ran: a Google token could not be had for this account. Approve again to retry."
        )
    );

    teardown(pg, &admin, &schema).await;
}

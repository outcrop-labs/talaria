// Live-DB proof of the refresh's two guarantees (cargo test -- --ignored).
// Both ARE Postgres's — a row lock and a committed delete — so no unit test
// can vouch for them. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-mcp-oauth --test refresh_live -- --ignored
//
//   single flight   two callers refreshing one expiring grant at once, against
//                   a provider that ROTATES refresh tokens (a spent one answers
//                   400 invalid_grant): one refresh reaches the provider, both
//                   callers get the fresh token, the grant survives, nothing
//                   is flagged.
//   refused grant   a provider answering 400 to the refresh: the token row is
//                   gone and `mcp_oauth_reconnect` carries the flag, whoever
//                   the caller was — the gateway's path, not only the health
//                   pass's.
//
// Each test builds its OWN schema with the three tables the refresh touches
// and points the pool's search_path at it alone, and the provider is a
// loopback stub on a std thread, so nothing real is read or written.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use talaria_secretbox::SecretBox;

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://talaria:talaria@127.0.0.1:5432/talaria".into())
}

fn test_box() -> SecretBox {
    let kek = talaria_secretbox::derive_kek("refresh-live-test");
    let dek = talaria_secretbox::new_dek().expect("entropy");
    SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
}

/// The provider stub: answers each token POST after a pause (so overlapping
/// refreshes really overlap), rotating the refresh token when `rotate` and
/// refusing everything when not. Answers (base url, refresh POSTs seen).
fn provider(rotate: bool) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(AtomicUsize::new(0));
    let valid = Arc::new(Mutex::new("r0".to_string()));
    let counter = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (counter, valid) = (counter.clone(), valid.clone());
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read the head, then the body Content-Length names.
                let body = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break text[end + 4..end + 4 + len].to_string();
                        }
                    }
                };
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(400));
                let offered = body
                    .split('&')
                    .find_map(|p| p.strip_prefix("refresh_token="))
                    .unwrap_or_default()
                    .to_string();
                let (status, answer) = {
                    let mut current = valid.lock().unwrap();
                    if rotate && offered == *current {
                        let n: u32 = current[1..].parse().unwrap_or(0) + 1;
                        *current = format!("r{n}");
                        (
                            "200 OK",
                            json!({ "access_token": format!("a{n}"), "refresh_token": format!("r{n}"),
                                    "expires_in": 3600, "token_type": "Bearer" }),
                        )
                    } else {
                        ("400 Bad Request", json!({ "error": "invalid_grant" }))
                    }
                };
                let text = answer.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
            });
        }
    });
    (base, seen)
}

/// A throwaway schema with the refresh's three tables, one OAuth server whose
/// token endpoint is the stub, and an org grant one second from expiry.
async fn scratch(tag: &str, base: &str, sb: &SecretBox) -> (PgPool, PgPool, String, String) {
    // SAFETY: every test in this binary sets the same value before its first
    // fetch; the stub is loopback, which safe_fetch refuses by default.
    unsafe { std::env::set_var("TALARIA_FETCH_ALLOW_HOSTS", "127.0.0.1") };
    let admin = PgPool::connect(&database_url())
        .await
        .expect("dev database");
    let schema = format!("mcp_refresh_live_{tag}_{}", std::process::id());
    let prefix = format!("mcp_refresh_live_{tag}_");
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
            "create table {schema}.mcp_servers (id uuid primary key, url text not null, oauth jsonb)"
        ),
        format!(
            "create table {schema}.mcp_oauth_tokens (server_id uuid not null, subject text not null, \
             tokens_enc text not null, updated_at timestamptz not null default now(), \
             primary key (server_id, subject))"
        ),
        format!(
            "create table {schema}.mcp_oauth_reconnect (server_id uuid not null, subject text not null, \
             detail text not null, flagged_at timestamptz not null default now(), \
             primary key (server_id, subject))"
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
        .max_connections(6)
        .connect_with(opts)
        .await
        .expect("scratch pool");
    let id = uuid::Uuid::new_v4().to_string();
    let oauth = json!({
        "tokenEndpoint": format!("{base}/token"),
        "resource": format!("{base}/mcp"),
        "client": { "id": "client-1", "redirectUri": "http://localhost/cb" },
    });
    sqlx::query("insert into mcp_servers (id, url, oauth) values ($1::uuid, $2, $3)")
        .bind(&id)
        .bind(format!("{base}/mcp"))
        .bind(&oauth)
        .execute(&pg)
        .await
        .expect("server row");
    let tokens = json!({
        "accessToken": "a0", "refreshToken": "r0", "tokenType": "Bearer",
        "expiresAt": talaria_agent_auth::now_ms() + 1_000,
    });
    sqlx::query("insert into mcp_oauth_tokens (server_id, subject, tokens_enc) values ($1::uuid, 'org', $2)")
        .bind(&id)
        .bind(sb.seal(&tokens.to_string()).unwrap())
        .execute(&pg)
        .await
        .expect("token row");
    (pg, admin, schema, id)
}

/// Close the scratch pool (its sessions hold the schema) and drop the schema.
async fn teardown(pg: PgPool, admin: &PgPool, schema: &str) {
    pg.close().await;
    // AssertSqlSafe: the schema name is this file's own tag and a pid.
    let sql = format!("drop schema if exists {schema} cascade");
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .execute(admin)
        .await
        .expect("drop the scratch schema");
}

async fn count(pg: &PgPool, table: &str, id: &str) -> i64 {
    let sql = format!("select count(*)::int8 from {table} where server_id::text = $1");
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_one(pg)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a live Postgres (DATABASE_URL)"]
async fn overlapping_refreshes_of_a_rotating_grant_are_one_refresh() {
    let sb = test_box();
    let (base, seen) = provider(true);
    let (pg, admin, schema, id) = scratch("flight", &base, &sb).await;

    let (a, b) = tokio::join!(
        talaria_mcp_oauth::oauth_token_for(&pg, &sb, &id, "org"),
        talaria_mcp_oauth::oauth_token_for(&pg, &sb, &id, "org"),
    );
    assert_eq!(a.unwrap().as_deref(), Some("a1"), "first caller");
    assert_eq!(
        b.unwrap().as_deref(),
        Some("a1"),
        "second caller reads the first's refresh"
    );
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "one refresh reached the provider"
    );
    assert_eq!(
        count(&pg, "mcp_oauth_tokens", &id).await,
        1,
        "the grant survives"
    );
    assert_eq!(
        count(&pg, "mcp_oauth_reconnect", &id).await,
        0,
        "nothing flagged"
    );
    let stored: String =
        sqlx::query_scalar("select tokens_enc from mcp_oauth_tokens where server_id::text = $1")
            .bind(&id)
            .fetch_one(&pg)
            .await
            .unwrap();
    let t: Value = serde_json::from_str(&sb.open(&stored).unwrap()).unwrap();
    assert_eq!(
        t["refreshToken"], "r1",
        "the rotated refresh token is the one kept"
    );

    teardown(pg, &admin, &schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a live Postgres (DATABASE_URL)"]
async fn a_refused_refresh_drops_the_grant_and_flags_it_for_reconnect() {
    let sb = test_box();
    let (base, _) = provider(false);
    let (pg, admin, schema, id) = scratch("refused", &base, &sb).await;

    let got = talaria_mcp_oauth::oauth_token_for(&pg, &sb, &id, "org").await;
    assert_eq!(got.unwrap(), None);
    assert_eq!(
        count(&pg, "mcp_oauth_tokens", &id).await,
        0,
        "the grant is gone"
    );
    let detail: Option<String> = sqlx::query_scalar(
        "select detail from mcp_oauth_reconnect where server_id::text = $1 and subject = 'org'",
    )
    .bind(&id)
    .fetch_optional(&pg)
    .await
    .unwrap();
    assert_eq!(detail.as_deref(), Some(talaria_mcp_oauth::REFRESH_REJECTED));

    teardown(pg, &admin, &schema).await;
}

// Shared handler state. Clone-cheap: PgPool and ConnectionManager are
// internally reference-counted, and the OnceCells mean the lazy handles are
// constructed at most once per process no matter how many clones race.
//
// The secretbox is the one lazy handle that may be loaded AGAIN: a box whose
// failure can heal (secret_keys not created yet, or not given its first key
// yet) is re-read at most every SECRETBOX_RETRY_AFTER. See `secretbox`.

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use sqlx::PgPool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use talaria_config::Config;
use talaria_secretbox::SecretBox;
use tokio::sync::OnceCell;

/// redis-rs 1.6 times out a multiplexed command at 500ms. This process has
/// one ConnectionManager — every lease heartbeat, publish, rate limit, and
/// session read shares it — and a fleet's pipeline blows past that while
/// Redis itself is fine. Failed renewals look like lost leases; runs stall
/// and agents retry into the same queue. Five seconds is patience for a busy
/// local Redis, not a hang: a dead server still fails, and a healthy command
/// answers in a millisecond so the ceiling never adds latency.
/// `TALARIA_REDIS_RESPONSE_TIMEOUT_MS` overrides; empty, zero, or garbage
/// keeps the default, same law as the pool knobs.
pub const DEFAULT_REDIS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn redis_response_timeout(raw: Option<&str>) -> Duration {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_REDIS_RESPONSE_TIMEOUT;
    };
    match raw.parse::<u64>() {
        Ok(0) => DEFAULT_REDIS_RESPONSE_TIMEOUT,
        Ok(ms) => Duration::from_millis(ms),
        Err(_) => DEFAULT_REDIS_RESPONSE_TIMEOUT,
    }
}

/// How long a box whose failure can heal is served before the next caller
/// reads `secret_keys` again. One cheap query per window, and only while the
/// box is failing — a healthy box and a root-secret mismatch never reload.
pub const SECRETBOX_RETRY_AFTER: Duration = Duration::from_secs(2);

/// The process's box and when it was last read from the database.
struct LoadedBox {
    sb: SecretBox,
    loaded: Instant,
}

impl LoadedBox {
    fn new(sb: SecretBox) -> Self {
        LoadedBox {
            sb,
            loaded: Instant::now(),
        }
    }

    fn due_for_retry(&self) -> bool {
        self.sb.failure_is_transient() && self.loaded.elapsed() >= SECRETBOX_RETRY_AFTER
    }
}

fn redis_manager_config() -> ConnectionManagerConfig {
    let timeout = redis_response_timeout(
        std::env::var("TALARIA_REDIS_RESPONSE_TIMEOUT_MS")
            .ok()
            .as_deref(),
    );
    ConnectionManagerConfig::new()
        .set_response_timeout(Some(timeout))
        .set_connection_timeout(Some(Duration::from_millis(2_000)))
}

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub cfg: Arc<Config>,
    redis: Arc<OnceCell<ConnectionManager>>,
    sb: Arc<OnceCell<RwLock<LoadedBox>>>,
    started: Instant,
}

impl AppState {
    pub fn new(pg: PgPool, cfg: Arc<Config>) -> Self {
        AppState {
            pg,
            cfg,
            redis: Arc::new(OnceCell::new()),
            sb: Arc::new(OnceCell::new()),
            started: Instant::now(),
        }
    }

    /// Swap in the rotated key set (admin.encryption).
    /// Only ever called after the re-encryption transaction committed, and
    /// only with a box whose DEKs include every version that transaction
    /// resealed. The cell is necessarily initialized by then (rotation had to
    /// read the old keys through it).
    pub fn install_secretbox(&self, next: SecretBox) {
        if let Some(lock) = self.sb.get() {
            *lock.write().expect("secretbox lock") = LoadedBox::new(next);
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// The shared Redis handle, connecting on first use. Errors — including a
    /// 2.5s timeout, the same bound healthz uses — propagate to the caller;
    /// rate limiting fails open, health reports degraded, per their own rules.
    pub async fn redis(&self) -> redis::RedisResult<ConnectionManager> {
        if let Some(conn) = self.redis.get() {
            return Ok(conn.clone());
        }
        // Client::open only parses the URL; ConnectionManager does the I/O and
        // owns reconnection from then on. The config is what keeps a busy
        // fleet off the library's 500ms command timeout — see
        // `redis_response_timeout`.
        let client = redis::Client::open(self.cfg.redis_url.as_str())?;
        let config = redis_manager_config();
        tracing::info!(
            "[state] redis response_timeout_ms={}",
            config
                .response_timeout()
                .unwrap_or(DEFAULT_REDIS_RESPONSE_TIMEOUT)
                .as_millis()
        );
        let connected = tokio::time::timeout(
            Duration::from_millis(2_500),
            ConnectionManager::new_with_config(client, config),
        )
        .await;
        match connected {
            Ok(Ok(conn)) => {
                let _ = self.redis.set(conn.clone());
                Ok(conn)
            }
            Ok(Err(e)) => Err(e),
            // redis 1.6 spells the client-side kind `Client` (the old
            // ClientError/IoError names are gone).
            Err(_) => Err(redis::RedisError::from((
                redis::ErrorKind::Client,
                "redis connect timeout",
            ))),
        }
    }

    /// The loaded secretbox, loading `secret_keys` on first use. SecretBox is
    /// Clone (plain key maps). A healthy box is loaded once per process — or
    /// swapped whole by a rotation (`install_secretbox`).
    ///
    /// A FAILED box is not always final. `talaria dev` starts this process
    /// before the app's migration pass creates `secret_keys` and its first
    /// key, and a fresh or reset database does the same in any order — so the
    /// first load could find no table, or an empty one, and a box cached in
    /// that state refused every seal (the first hire's agent key included)
    /// until someone restarted the api. A failure that can heal
    /// (`SecretBox::failure_is_transient`) is read again, by the first caller
    /// after SECRETBOX_RETRY_AFTER, while everyone else keeps the box they
    /// have; a root that cannot unwrap the keys is never retried — that is a
    /// different root secret, and it stays loud.
    pub async fn secretbox(&self) -> Result<SecretBox, String> {
        let cell = self
            .sb
            .get_or_try_init(|| async {
                Ok::<_, String>(RwLock::new(LoadedBox::new(
                    SecretBox::load(&self.pg, self.cfg.secret_root.material()).await,
                )))
            })
            .await?;
        {
            let slot = cell.read().expect("secretbox lock");
            if !slot.due_for_retry() {
                return Ok(slot.sb.clone());
            }
        }
        // Claim the retry under the write lock, so one caller reads the table
        // and the rest are served the current box until it answers.
        let failed = {
            let mut slot = cell.write().expect("secretbox lock");
            if !slot.due_for_retry() {
                return Ok(slot.sb.clone());
            }
            slot.loaded = Instant::now();
            slot.sb.clone()
        };
        let next = failed.reload(&self.pg).await;
        let mut slot = cell.write().expect("secretbox lock");
        // Replace only the box that was retried: a rotation may have
        // installed a newer one meanwhile, and that one wins.
        if slot.sb.failure_is_transient() {
            match next.failure() {
                None => tracing::info!(
                    "[secretbox] secret_keys loaded on retry — sealing works without a restart"
                ),
                Some(why) if !next.failure_is_transient() => {
                    tracing::error!("[secretbox] secret_keys reloaded and still unusable: {why}")
                }
                Some(_) => {}
            }
            *slot = LoadedBox::new(next);
        }
        Ok(slot.sb.clone())
    }
}

/// A config for the tests below: the given database and secret root, and a
/// dead redis (nothing here needs one).
#[cfg(test)]
fn test_config(database_url: &str, root: &str) -> Arc<Config> {
    Arc::new(
        Config::from_parts(
            database_url.into(),
            "redis://127.0.0.1:1".into(),
            root.into(),
            String::new(),
            String::new(),
            String::new(),
        )
        .expect("test config assembles"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_timeout_rejects_the_library_default() {
        // THE BUG. redis-rs 1.6's DEFAULT_RESPONSE_TIMEOUT is 500ms. A test
        // that only checks "some Duration" would pass on the library default.
        assert_eq!(redis_response_timeout(None), Duration::from_secs(5));
        assert_ne!(redis_response_timeout(None), Duration::from_millis(500));
        assert_eq!(redis_response_timeout(Some("")), Duration::from_secs(5));
        assert_eq!(redis_response_timeout(Some("0")), Duration::from_secs(5));
        assert_eq!(redis_response_timeout(Some("nope")), Duration::from_secs(5));
        assert_eq!(
            redis_response_timeout(Some("2500")),
            Duration::from_millis(2500)
        );
    }

    #[tokio::test]
    async fn a_box_loaded_before_postgres_answers_is_retried_not_kept() {
        // Nothing listens on port 1: the read fails the way it does while the
        // database is still coming up. No database needed.
        let url = "postgres://talaria:talaria@127.0.0.1:1/talaria";
        let pg = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(300))
            .connect_lazy(url)
            .unwrap();
        let state = AppState::new(pg, test_config(url, "root"));
        let first = state.secretbox().await.unwrap();
        assert!(first.failure_is_transient(), "{:?}", first.failure());
        let loaded_at = state.sb.get().unwrap().read().unwrap().loaded;
        // Inside the window: the cached box, no second read.
        let _ = state.secretbox().await.unwrap();
        assert_eq!(state.sb.get().unwrap().read().unwrap().loaded, loaded_at);
        // Past it: the next caller reads again (and stamps a new load time).
        tokio::time::sleep(SECRETBOX_RETRY_AFTER + Duration::from_millis(50)).await;
        let again = state.secretbox().await.unwrap();
        assert!(again.failure_is_transient());
        assert!(state.sb.get().unwrap().read().unwrap().loaded > loaded_at);
        assert_eq!(again.kek(), first.kek(), "a reload keeps the derived KEK");
    }
}

// ── The retry against a real Postgres (follow-up A6) ─────────────────────────
//
//   DATABASE_URL=postgres://… cargo test -p talaria-state --lib -- --ignored live_db
//
// CI's live-tests job runs these (`cargo test --workspace --lib -- --ignored`).
// Each test works in a schema of its own, reached through search_path, so the
// `secret_keys` it creates, fills and drops is never the database's real one.

#[cfg(test)]
mod live_db {
    use super::*;
    use std::collections::HashMap;
    use talaria_secretbox::{IV_LEN, derive_kek};

    const ROOT: &str = "talaria-state-live-test-root";

    /// A pool whose every connection resolves unqualified names in a fresh,
    /// empty schema — and the schema's name, for the cleanup.
    async fn isolated() -> (PgPool, String, String) {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let schema = format!("sb_retry_{}_{nanos}", std::process::id());
        let admin = PgPool::connect(&url).await.expect("connect");
        // AssertSqlSafe: the name is this function's own digits and underscores.
        sqlx::query(sqlx::AssertSqlSafe(format!("create schema {schema}")))
            .execute(&admin)
            .await
            .unwrap();
        let path = schema.clone();
        let pg = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(
                        conn,
                        sqlx::AssertSqlSafe(format!("set search_path to {path}")),
                    )
                    .await
                    .map(|_| ())
                })
            })
            .connect(&url)
            .await
            .expect("connect");
        (pg, schema, url)
    }

    async fn drop_schema(url: &str, schema: &str) {
        let admin = PgPool::connect(url).await.expect("connect");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "drop schema if exists {schema} cascade"
        )))
        .execute(&admin)
        .await
        .unwrap();
    }

    /// The table exactly as the app's migration pass creates it
    /// (ui/src/server/db/pg.ts).
    async fn create_table(pg: &PgPool) {
        sqlx::query(
            "create table if not exists secret_keys (
               version int primary key,
               wrapped_dek text not null,
               active boolean not null default true,
               created_at timestamptz not null default now()
             )",
        )
        .execute(pg)
        .await
        .unwrap();
    }

    async fn insert_key(pg: &PgPool, root: &str, version: i32) {
        let wrapped = SecretBox::wrap_dek_with_iv(
            &derive_kek(root),
            &[version as u8 + 40; 32],
            &[version as u8; IV_LEN],
        )
        .unwrap();
        sqlx::query("insert into secret_keys (version, wrapped_dek, active) values ($1, $2, true)")
            .bind(version)
            .bind(wrapped)
            .execute(pg)
            .await
            .unwrap();
    }

    async fn past_the_window() {
        tokio::time::sleep(SECRETBOX_RETRY_AFTER + Duration::from_millis(100)).await;
    }

    #[tokio::test]
    #[ignore = "needs a Postgres (DATABASE_URL)"]
    async fn the_first_key_written_after_boot_is_picked_up_without_a_restart() {
        let (pg, schema, url) = isolated().await;
        let state = AppState::new(pg.clone(), test_config(&url, ROOT));

        // Boot: the api asks before the migration pass created the table.
        let sb = state.secretbox().await.unwrap();
        assert!(sb.failure().unwrap().starts_with("secret_keys unreadable"));
        assert!(sb.failure_is_transient());
        assert!(sb.seal("tlk_agent_key").is_err(), "the bug's symptom");

        // The pass creates the table: still no key, still transient.
        create_table(&pg).await;
        past_the_window().await;
        let sb = state.secretbox().await.unwrap();
        assert!(sb.failure().unwrap().starts_with("secret_keys is empty"));
        assert!(sb.failure_is_transient());

        // ...and writes the first key. The next read past the window is a
        // healthy box, in the same process, with nobody restarting it.
        insert_key(&pg, ROOT, 1).await;
        let cached = state.secretbox().await.unwrap();
        assert!(
            cached.failure().is_some(),
            "inside the window: the cached box"
        );
        past_the_window().await;
        let sb = state.secretbox().await.unwrap();
        assert!(sb.failure().is_none(), "{:?}", sb.failure());
        let token = sb.seal("tlk_agent_key").unwrap();
        assert!(token.starts_with("v2:1:"), "{token}");
        assert_eq!(sb.open(&token).unwrap(), "tlk_agent_key");

        // Healthy is final: a later table change is a rotation's business.
        let loaded = state.sb.get().unwrap().read().unwrap().loaded;
        past_the_window().await;
        let _ = state.secretbox().await.unwrap();
        assert_eq!(state.sb.get().unwrap().read().unwrap().loaded, loaded);

        drop_schema(&url, &schema).await;
    }

    #[tokio::test]
    #[ignore = "needs a Postgres (DATABASE_URL)"]
    async fn a_root_that_cannot_unwrap_the_keys_stays_failed_and_is_not_reread() {
        let (pg, schema, url) = isolated().await;
        create_table(&pg).await;
        insert_key(&pg, "the-root-that-sealed-this-database", 1).await;
        let state = AppState::new(pg.clone(), test_config(&url, ROOT));

        let sb = state.secretbox().await.unwrap();
        assert!(sb.failure().unwrap().contains("none can be unwrapped"));
        assert!(!sb.failure_is_transient());
        let loaded = state.sb.get().unwrap().read().unwrap().loaded;

        // Even a key this root COULD read, added later, is not picked up: the
        // box does not reread on a mismatch, so there is no reload storm and
        // the diagnosis a person must act on stays in place.
        insert_key(&pg, ROOT, 2).await;
        past_the_window().await;
        for _ in 0..5 {
            let sb = state.secretbox().await.unwrap();
            assert!(sb.failure().unwrap().contains("none can be unwrapped"));
        }
        assert_eq!(state.sb.get().unwrap().read().unwrap().loaded, loaded);

        drop_schema(&url, &schema).await;
    }

    fn loaded_at(state: &AppState) -> Instant {
        state.sb.get().unwrap().read().unwrap().loaded
    }

    #[tokio::test]
    #[ignore = "needs a Postgres (DATABASE_URL)"]
    async fn a_rotation_installed_while_a_retry_reads_is_not_overwritten() {
        let (pg, schema, url) = isolated().await;
        let state = AppState::new(pg.clone(), test_config(&url, ROOT));
        assert!(state.secretbox().await.unwrap().failure_is_transient());
        let first_load = loaded_at(&state);

        // Hold both connections `isolated` lets the pool open, so the retry's
        // read of secret_keys waits on acquire: the window between claiming a
        // retry and installing what it read, which is where a rotation can land.
        let held = (pg.acquire().await.unwrap(), pg.acquire().await.unwrap());
        past_the_window().await;
        let retry = tokio::spawn({
            let state = state.clone();
            async move { state.secretbox().await }
        });
        // Claiming the retry stamps a new load time before the read begins.
        while loaded_at(&state) == first_load {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // A rotation commits and installs its box while that read waits...
        let mut deks = HashMap::new();
        deks.insert(7u32, [7u8; 32]);
        state.install_secretbox(SecretBox::from_parts(derive_kek(ROOT), deks, Some(7)));
        // ...then the read answers. This schema still has no table, so what it
        // read is a failed box, and installing it would undo the rotation.
        drop(held);
        let sb = retry.await.unwrap().unwrap();
        assert_eq!(sb.active_key_version(), Some(7), "{:?}", sb.failure());
        let sb = state.secretbox().await.unwrap();
        assert!(sb.failure().is_none(), "{:?}", sb.failure());
        assert_eq!(sb.active_key_version(), Some(7));

        drop_schema(&url, &schema).await;
    }
}

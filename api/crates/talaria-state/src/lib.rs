// Shared handler state. Clone-cheap: PgPool and ConnectionManager are
// internally reference-counted, and the OnceCells mean the lazy handles are
// constructed at most once per process no matter how many clones race.

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
    sb: Arc<OnceCell<RwLock<SecretBox>>>,
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
            *lock.write().expect("secretbox lock") = next;
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
    /// Clone (plain key maps) and loaded at most once per process — or swapped
    /// whole by a rotation, which is the one writer of the lock inside.
    pub async fn secretbox(&self) -> Result<SecretBox, String> {
        let cell = self
            .sb
            .get_or_try_init(|| async {
                Ok::<_, String>(RwLock::new(
                    SecretBox::load(&self.pg, self.cfg.secret_root.material()).await,
                ))
            })
            .await?;
        Ok(cell.read().expect("secretbox lock").clone())
    }
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
}

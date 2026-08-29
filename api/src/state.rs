// Shared handler state. Clone-cheap: PgPool and ConnectionManager are
// internally reference-counted, and the OnceCells mean the lazy handles are
// constructed at most once per process no matter how many clones race.

use crate::config::Config;
use crate::secretbox::SecretBox;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub cfg: Arc<Config>,
    /// Lazily-connected Redis. ConnectionManager reconnects on its own once
    /// established; the OnceCell covers the FIRST connection, so the process
    /// can boot while Redis is still coming up and healthz reports degraded
    /// instead of the process dying (same posture as the TS side's lazy
    /// getRedis()).
    redis: Arc<OnceCell<ConnectionManager>>,
    /// Lazily-loaded secretbox keys. Nothing in the models slice needs a key;
    /// the first operation that does pays the load (and sees its diagnosis if
    /// the root secret is wrong — recorded, never thrown; secretbox.rs).
    /// DELETE THIS ALLOW with the phase-2 relay, its first caller.
    #[allow(dead_code)]
    sb: Arc<OnceCell<SecretBox>>,
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
        // owns reconnection from then on.
        let client = redis::Client::open(self.cfg.redis_url.as_str())?;
        let connected =
            tokio::time::timeout(Duration::from_millis(2_500), ConnectionManager::new(client))
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
    /// Clone (plain key maps) and loaded at most once per process.
    /// DELETE THIS ALLOW with the phase-2 relay, its first caller.
    #[allow(dead_code)]
    pub async fn secretbox(&self) -> Result<SecretBox, String> {
        self.sb
            .get_or_try_init(|| async {
                Ok::<_, String>(SecretBox::load(&self.pg, self.cfg.secret_root.material()).await)
            })
            .await
            .cloned()
    }
}

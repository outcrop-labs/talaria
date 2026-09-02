// Shared handler state. Clone-cheap: PgPool and ConnectionManager are
// internally reference-counted, and the OnceCells mean the lazy handles are
// constructed at most once per process no matter how many clones race.

use crate::config::Config;
use crate::secretbox::SecretBox;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub cfg: Arc<Config>,
    /// Lazily-connected Redis. ConnectionManager reconnects on its own once
    /// established; the OnceCell covers the FIRST connection, so the process
    /// can boot while Redis is still coming up and healthz reports degraded
    /// instead of the process dying.
    redis: Arc<OnceCell<ConnectionManager>>,
    /// Lazily-loaded secretbox keys. Nothing in the models slice needs a key;
    /// the chat relay unseals endpoint credentials through it. The RwLock
    /// inside the cell is for ROTATION (admin.encryption): the whole key set
    /// is swapped in one write after the re-encryption transaction commits —
    /// a write a bare OnceCell cannot take.
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

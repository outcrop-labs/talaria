// Retention: what the notification plane deletes on its own, daily.
//
//   · NOTIFICATION ROWS (SE-13, O-15): read for more than 90 days, or unread
//     for more than 180, in batches of 10,000 — and the `inbox_focus_state`
//     rows that pointed at them (keyed by the row id as text, with no foreign
//     key to cascade: PLAN2 §1.2 finding 10).
//   · PUSH SUBSCRIPTIONS (C26, WP-7 expiry): a row filed by a session that has
//     since ended (logged out, or simply expired after its 7 days) keeps
//     reaching that browser with the person's notifications. The session
//     store writes a liveness marker `sesshash:<hex sha256 of the session id>`
//     beside every session, with the session's own TTL (talaria-session); a
//     row whose `session_hash` has no marker is deleted. Before reading the
//     markers, the pass backfills one for every live session that lacks it —
//     sessions minted before the marker existed, or by another writer — so a
//     deploy never mistakes a live session for a dead one. Rows from before
//     wave 1 (no `session_hash`) go after 30 days with no delivery, and a row
//     that has failed ten times with no delivery for seven days goes too.
//
// Both are leased scheduler jobs (rows every instance can reach). Without the
// boot-installed Redis handle the session pass is skipped, never guessed:
// fail closed toward keeping a subscription.

use std::sync::{Arc, LazyLock};

use redis::AsyncCommands;
use sqlx::PgPool;

/// Read rows older than this go.
pub const READ_RETENTION_DAYS: i32 = 90;
/// Unread rows older than this go.
pub const UNREAD_RETENTION_DAYS: i32 = 180;
/// Rows one delete takes.
pub const RETENTION_BATCH: i64 = 10_000;
/// A pre-wave-1 subscription (no session hash) with no delivery for this long.
pub const UNBOUND_SUBSCRIPTION_DAYS: i32 = 30;
/// A subscription failed this many times in a row…
pub const FAILURES_TO_PRUNE: i32 = 10;
/// …with no delivery for this long.
pub const FAILING_SUBSCRIPTION_DAYS: i32 = 7;

/// The session liveness marker's key prefix. talaria-session writes
/// `sesshash:<hex sha256 of the session id>` with the session's TTL; the
/// prefix and the hash are the contract between the two crates, pinned by a
/// test on each side.
pub const SESSION_MARKER_PREFIX: &str = "sesshash:";

/// The marker key for a session id.
pub fn session_marker_for_sid(sid: &str) -> String {
    use sha2::Digest as _;
    let hex: String = sha2::Sha256::digest(sid.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{SESSION_MARKER_PREFIX}{hex}")
}

/// TEST ONLY: `TALARIA_TEST_NOTIFY_DAILY_EVERY_MS=<n>` runs the two daily jobs
/// every n ms (first run after n ms), so a live run can watch them. Read once;
/// loud at registration.
fn test_daily_every_ms() -> Option<u64> {
    static V: LazyLock<Option<u64>> = LazyLock::new(|| {
        std::env::var("TALARIA_TEST_NOTIFY_DAILY_EVERY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v >= 1_000)
    });
    *V
}

const DAY_MS: u64 = 24 * 60 * 60_000;

/// One retention pass: aged rows and their inbox-focus rows, batch by batch.
/// Answers how many notification rows went.
pub async fn retention_pass(pg: &PgPool) -> Result<u64, sqlx::Error> {
    let mut total = 0u64;
    loop {
        let (gone, _focus): (i64, i64) = sqlx::query_as(
            "with aged as ( \
               select id from notifications \
                where (read_at is not null and read_at < now() - $1::int * interval '1 day') \
                   or (read_at is null and created_at < now() - $2::int * interval '1 day') \
                limit $3), \
             gone as ( \
               delete from notifications n using aged where n.id = aged.id \
               returning n.id::text as id, n.user_id), \
             focus as ( \
               delete from inbox_focus_state f using gone \
                where f.user_id = gone.user_id and f.source_type = 'notification' \
                  and f.source_id = gone.id \
               returning 1) \
             select (select count(*) from gone), (select count(*) from focus)",
        )
        .bind(READ_RETENTION_DAYS)
        .bind(UNREAD_RETENTION_DAYS)
        .bind(RETENTION_BATCH)
        .fetch_one(pg)
        .await?;
        total += gone as u64;
        if gone < RETENTION_BATCH {
            return Ok(total);
        }
    }
}

/// The retention job's work.
pub async fn run_retention(pg: &PgPool) -> Result<Option<String>, String> {
    let gone = retention_pass(pg)
        .await
        .map_err(|e| format!("the notification retention pass failed: {e}"))?;
    Ok((gone > 0).then(|| {
        format!(
            "deleted {gone} notification(s) past retention (read > {READ_RETENTION_DAYS} d, unread > {UNREAD_RETENTION_DAYS} d)"
        )
    }))
}

/// What one prune pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrunePass {
    /// Markers written for live sessions that had none.
    pub backfilled: usize,
    /// Rows whose session has ended.
    pub ended: u64,
    /// Pre-wave-1 rows with no delivery for 30 days.
    pub unbound: u64,
    /// Rows failing for a week.
    pub failing: u64,
    /// The session pass did not run (no Redis).
    pub sessions_skipped: bool,
}

/// Write a marker for every live session that lacks one, with the session's
/// own remaining TTL.
async fn backfill_markers(conn: &mut redis::aio::ConnectionManager) -> redis::RedisResult<usize> {
    let mut cursor: u64 = 0;
    let mut written = 0usize;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .cursor_arg(cursor)
            .arg("MATCH")
            .arg("sess:*")
            .arg("COUNT")
            .arg(200)
            .query_async(conn)
            .await?;
        for key in keys {
            let Some(sid) = key.strip_prefix("sess:").filter(|s| !s.is_empty()) else {
                continue;
            };
            let marker = session_marker_for_sid(sid);
            let exists: bool = conn.exists(&marker).await?;
            if exists {
                continue;
            }
            let ttl: i64 = redis::cmd("PTTL").arg(&key).query_async(conn).await?;
            let set: Option<String> = match ttl {
                t if t > 0 => {
                    redis::cmd("SET")
                        .arg(&marker)
                        .arg("1")
                        .arg("PX")
                        .arg(t)
                        .arg("NX")
                        .query_async(conn)
                        .await?
                }
                -1 => {
                    redis::cmd("SET")
                        .arg(&marker)
                        .arg("1")
                        .arg("NX")
                        .query_async(conn)
                        .await?
                }
                _ => None, // gone between the scan and now
            };
            if set.is_some() {
                written += 1;
            }
        }
        if next == 0 {
            return Ok(written);
        }
        cursor = next;
    }
}

/// The session hashes, of those given, that have no live marker.
async fn ended_sessions(
    conn: &mut redis::aio::ConnectionManager,
    hashes: &[String],
) -> redis::RedisResult<Vec<String>> {
    let mut dead = Vec::new();
    for chunk in hashes.chunks(100) {
        let mut pipe = redis::pipe();
        for h in chunk {
            pipe.exists(format!("{SESSION_MARKER_PREFIX}{h}"));
        }
        let alive: Vec<bool> = pipe.query_async(conn).await?;
        for (h, a) in chunk.iter().zip(alive) {
            if !a {
                dead.push(h.clone());
            }
        }
    }
    Ok(dead)
}

/// One prune pass over `push_subscriptions`. `redis` is the boot-installed
/// handle; without it only the database-only rules run.
pub async fn push_prune_pass(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
) -> Result<PrunePass, String> {
    let mut pass = PrunePass::default();
    match redis {
        Some(mut conn) => {
            pass.backfilled = backfill_markers(&mut conn)
                .await
                .map_err(|e| format!("the session marker backfill failed: {e}"))?;
            let hashes: Vec<String> = sqlx::query_scalar(
                "select distinct session_hash from push_subscriptions \
                 where session_hash is not null",
            )
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
            let dead = ended_sessions(&mut conn, &hashes)
                .await
                .map_err(|e| format!("the session marker read failed: {e}"))?;
            if !dead.is_empty() {
                pass.ended =
                    sqlx::query("delete from push_subscriptions where session_hash = any($1)")
                        .bind(&dead)
                        .execute(pg)
                        .await
                        .map_err(|e| e.to_string())?
                        .rows_affected();
            }
        }
        None => pass.sessions_skipped = true,
    }
    pass.unbound = sqlx::query(
        "delete from push_subscriptions where session_hash is null \
           and last_seen_at < now() - $1::int * interval '1 day'",
    )
    .bind(UNBOUND_SUBSCRIPTION_DAYS)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?
    .rows_affected();
    pass.failing = sqlx::query(
        "delete from push_subscriptions where failures >= $1 \
           and last_seen_at < now() - $2::int * interval '1 day'",
    )
    .bind(FAILURES_TO_PRUNE)
    .bind(FAILING_SUBSCRIPTION_DAYS)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?
    .rows_affected();
    Ok(pass)
}

/// The prune job's work, over the boot-installed Redis handle.
pub async fn run_push_prune(pg: &PgPool) -> Result<Option<String>, String> {
    let pass = push_prune_pass(pg, crate::presence::presence_redis()).await?;
    let mut parts = Vec::new();
    if pass.ended > 0 {
        parts.push(format!("{} whose session ended", pass.ended));
    }
    if pass.unbound > 0 {
        parts.push(format!(
            "{} pre-session row(s) idle {UNBOUND_SUBSCRIPTION_DAYS} d",
            pass.unbound
        ));
    }
    if pass.failing > 0 {
        parts.push(format!(
            "{} failing for {FAILING_SUBSCRIPTION_DAYS} d",
            pass.failing
        ));
    }
    let mut msg = if parts.is_empty() {
        None
    } else {
        Some(format!("pruned push subscriptions: {}", parts.join(", ")))
    };
    if pass.backfilled > 0 {
        let m = msg.get_or_insert_with(|| "pruned nothing".into());
        m.push_str(&format!(
            "; wrote {} session marker(s) for live sessions",
            pass.backfilled
        ));
    }
    if pass.sessions_skipped {
        let m = msg.get_or_insert_with(String::new);
        m.push_str(" (no Redis handle: rows of ended sessions were kept)");
    }
    Ok(msg)
}

fn daily_spec(
    name: talaria_scheduler::JobName,
    first_ms: u64,
    run: talaria_scheduler::JobFn,
) -> talaria_scheduler::JobSpec {
    let fast = test_daily_every_ms();
    talaria_scheduler::JobSpec {
        name,
        every_ms: fast.unwrap_or(DAY_MS),
        first_run_delay_ms: Some(fast.unwrap_or(first_ms)),
        max_run_ms: Some(15 * 60_000),
        per_instance: false,
        run,
    }
}

pub fn retention_job_spec(pg: PgPool) -> talaria_scheduler::JobSpec {
    daily_spec(
        talaria_scheduler::JobName::NotifyRetention,
        10 * 60_000,
        Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { run_retention(&pg).await })
        }),
    )
}

pub fn push_prune_job_spec(pg: PgPool) -> talaria_scheduler::JobSpec {
    daily_spec(
        talaria_scheduler::JobName::PushPrune,
        15 * 60_000,
        Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { run_push_prune(&pg).await })
        }),
    )
}

/// Register both daily jobs.
pub fn register_retention_jobs(pg: PgPool) {
    if let Some(ms) = test_daily_every_ms() {
        tracing::warn!(
            "[notifications] TALARIA_TEST_NOTIFY_DAILY_EVERY_MS={ms}: retention and the push prune run every {ms} ms. TEST ONLY."
        );
    }
    talaria_scheduler::register_job(retention_job_spec(pg.clone()));
    talaria_scheduler::register_job(push_prune_job_spec(pg));
}

#[cfg(test)]
mod tests {
    use super::*;

    // The marker key is the contract with talaria-session: the prefix, then
    // the lowercase hex SHA-256 of the session id (talaria_session::
    // session_hash), the same string push_subscriptions.session_hash holds.
    #[test]
    fn the_marker_key_is_the_session_hash() {
        assert_eq!(
            session_marker_for_sid("abc"),
            "sesshash:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(session_marker_for_sid("x").len(), "sesshash:".len() + 64);
    }

    #[tokio::test]
    async fn the_retention_window_is_the_owners() {
        assert_eq!((READ_RETENTION_DAYS, UNREAD_RETENTION_DAYS), (90, 180));
        assert_eq!(RETENTION_BATCH, 10_000);
        let spec = retention_job_spec(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://x@localhost/x")
                .unwrap(),
        );
        if test_daily_every_ms().is_none() {
            assert_eq!(spec.every_ms, DAY_MS);
        }
        assert!(!spec.per_instance, "retention is leased");
    }
}

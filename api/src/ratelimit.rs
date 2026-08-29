// Rate limiting — port of ui/src/server/rate-limit.ts. Redis-backed so a
// counter survives a restart and holds across BOTH runtimes during the
// migration (same key space, same window arithmetic). A fixed window (INCR +
// EXPIRE on first hit), not a sliding one: it can let through up to 2x the
// limit across a window boundary, which is the right trade for a brake whose
// goal is "thousands become dozens", not exact accounting.
//
// Unit-tested now; wired into the chat-completions route's rpm brake.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

pub struct RateLimitResult {
    /// False when the caller should be refused.
    pub ok: bool,
    /// Seconds until the window resets — send as Retry-After.
    pub retry_after: i64,
}

/// The Redis key for a limiter name. Byte-identical to the TS spelling so the
/// two runtimes share one counter, whatever is serving the route today.
pub fn redis_key(key: &str) -> String {
    format!("talaria:rl:{key}")
}

/// Count one hit against `key`. Fails OPEN if Redis is unreachable: a limiter
/// outage must not lock everyone out of their own deployment.
pub async fn rate_limit(
    redis: &mut ConnectionManager,
    key: &str,
    limit: i64,
    window_seconds: i64,
) -> RateLimitResult {
    let k = redis_key(key);
    let counted = async {
        let count: i64 = redis.incr(&k, 1).await?;
        if count == 1 {
            let _: bool = redis.expire(&k, window_seconds).await?;
        }
        if count <= limit {
            return Ok::<_, redis::RedisError>(RateLimitResult {
                ok: true,
                retry_after: 0,
            });
        }
        let ttl: i64 = redis.ttl(&k).await?;
        Ok(RateLimitResult {
            ok: false,
            retry_after: if ttl > 0 { ttl } else { window_seconds },
        })
    };
    match counted.await {
        Ok(r) => r,
        Err(_) => RateLimitResult {
            ok: true,
            retry_after: 0,
        },
    }
}

/// Drop the counter for `key` (a success shouldn't leave the budget spent).
/// No Rust caller yet — the TS routes that use it are a later batch.
#[allow(dead_code)]
pub async fn rate_limit_reset(redis: &mut ConnectionManager, key: &str) {
    let _: Result<i64, _> = redis.del(redis_key(key)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shape_matches_the_ts_limiter() {
        // The gateway chat route's key is `gw:key:{keyId}` in TS
        // (llm.v1.chat.completions.ts); the shared counter depends on this
        // exact string.
        assert_eq!(redis_key("gw:key:abc"), "talaria:rl:gw:key:abc");
    }
}

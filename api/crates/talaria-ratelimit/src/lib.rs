// Rate limiting. Redis-backed so a
// counter survives a restart. A fixed window (INCR +
// EXPIRE on first hit), not a sliding one: it can let through up to 2x the
// limit across a window boundary, which is the right trade for a brake whose
// goal is "thousands become dozens", not exact accounting.
//
// Wired into the chat-completions route's rpm brake, the login/claim/join
// doors, and the agent write doors (`brake` below).

use axum::http::StatusCode;
use axum::response::Response;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use talaria_error::house_error;

pub struct RateLimitResult {
    /// False when the caller should be refused.
    pub ok: bool,
    /// Seconds until the window resets — send as Retry-After.
    pub retry_after: i64,
}

/// The Redis key for a limiter name — `talaria:rl:` prefix, a stable shape.
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

/// The house 429: the house error shape with the limiter's Retry-After
/// beside it. One body, here, because every door that refuses on a budget
/// needs exactly this and a copy per route crate is a copy that drifts.
pub fn too_many_requests(retry_after: i64, message: &str) -> Response {
    let mut resp = house_error(StatusCode::TOO_MANY_REQUESTS, message);
    if let Ok(v) = http::HeaderValue::from_str(&retry_after.to_string()) {
        resp.headers_mut().insert(http::header::RETRY_AFTER, v);
    }
    resp
}

/// Count one hit for `key` against a budget; `Some(429)` when it is spent,
/// carrying `message`. The agent write doors' one-liner — the caller holds
/// the Redis handle (and skips the brake, failing open, when it has none).
pub async fn brake(
    redis: &mut ConnectionManager,
    key: &str,
    limit: i64,
    window_seconds: i64,
    message: &str,
) -> Option<Response> {
    let hit = rate_limit(redis, key, limit, window_seconds).await;
    (!hit.ok).then(|| too_many_requests(hit.retry_after, message))
}

/// Drop the counter for `key` (a success shouldn't leave the budget spent).
pub async fn rate_limit_reset(redis: &mut ConnectionManager, key: &str) {
    let _: Result<i64, _> = redis.del(redis_key(key)).await;
}

/// The client's address, for limiting purposes only.
///
/// X-Forwarded-For is caller-supplied and trivially rotated, so trusting it
/// blindly turns a per-IP limit into no limit at all. It is only honored when
/// the operator states that a proxy really is in front (TALARIA_TRUST_PROXY),
/// because only then is the left-most entry something the client can't freely
/// invent.
///
/// With no trusted proxy this returns a constant, and callers lean on the
/// per-identity limit instead — deliberate: a wrong IP is worse than no IP,
/// because it silently partitions the counter per attacker-chosen value.
pub fn client_ip(headers: &http::HeaderMap) -> String {
    let trust = std::env::var("TALARIA_TRUST_PROXY")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if trust.is_empty() || trust == "0" || trust == "false" || trust == "no" {
        return "direct".into();
    }
    let xff = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    if let Some(first) = xff
        .and_then(|x| x.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return first.to_string();
    }
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("direct")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limiter_key_carries_the_shared_prefix() {
        // The gateway chat route's key is `gw:key:{keyId}`;
        // the shared counter depends on this exact string.
        assert_eq!(redis_key("gw:key:abc"), "talaria:rl:gw:key:abc");
    }
}

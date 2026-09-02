// The Postgres pool.
//
// THE SCHEMA IS MANAGED OUTSIDE THIS CRATE. This service reads and writes
// existing tables and never runs sqlx::migrate!, issues DDL, or writes to
// schema_migrations — the migration history is owned and applied elsewhere
// (advisory lock 8_314_207 guards it).

use crate::config::Config;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::str::FromStr;
use std::time::Duration;

/// Pool ceiling when the env is silent. Each instance's postgres sidecar
/// ships default `max_connections = 100`; this pool at 40 plus the UI's 20
/// plus its one-shot migration connection lands ≈ 61 of 100. The inherited
/// TS ceiling was 10, which agent-fleet load queued into acquire timeouts —
/// the pool, not any rate limiter, is what throttled the fleet.
pub const DEFAULT_PG_POOL_MAX: u32 = 40;

/// A raw value parsed with a fallback — absent, empty, or garbage keeps the
/// default, so a typo'd env never takes the process down.
fn parse_num<T: FromStr>(raw: &str, default: T) -> T {
    raw.trim().parse().ok().unwrap_or(default)
}

fn env_num<T: FromStr>(name: &str, default: T) -> T {
    match std::env::var(name) {
        Ok(v) => parse_num(&v, default),
        _ => default,
    }
}

/// The resolved knobs — `pool` applies them and healthz reports `max` from
/// the same source, so what runs and what is reported can never disagree.
pub struct PoolTuning {
    pub max: u32,
    pub acquire_ms: u64,
}

fn tuning() -> PoolTuning {
    PoolTuning {
        max: env_num("TALARIA_PG_POOL_MAX", DEFAULT_PG_POOL_MAX).clamp(1, 200),
        acquire_ms: env_num("TALARIA_PG_ACQUIRE_TIMEOUT_MS", 15_000),
    }
}

/// The resolved ceiling, for healthz.
pub fn configured_pool_max() -> u32 {
    tuning().max
}

pub fn pool(cfg: &Config) -> PgPool {
    let t = tuning();
    // The running ceiling belongs in the log, not just in the env.
    tracing::info!(
        "[db] pg pool max={} acquire_timeout_ms={}",
        t.max,
        t.acquire_ms
    );
    // connect_lazy: construction does no I/O, so the api boots before
    // postgres and its healthz reports degraded instead of the process
    // dying — only healthz's round trip tells the truth.
    //
    // The acquire timeout is patience, not a brake: the TS pool queued
    // without bound, so 5s here turned every burst into a wall of 500s.
    // 15s lets a spike queue through; a genuinely wedged postgres still
    // surfaces via healthz long before requests give up.
    PgPoolOptions::new()
        .max_connections(t.max)
        .acquire_timeout(Duration::from_millis(t.acquire_ms))
        .connect_lazy(&cfg.database_url)
        .expect("DATABASE_URL passed Config's scheme check but sqlx rejects it")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_num_falls_back_on_empty_whitespace_and_garbage() {
        assert_eq!(parse_num("", 40_u32), 40);
        assert_eq!(parse_num("   ", 40_u32), 40);
        assert_eq!(parse_num("forty", 40_u32), 40);
        assert_eq!(parse_num("4.0", 40_u32), 40);
    }

    #[test]
    fn parse_num_honors_the_value_including_padding() {
        assert_eq!(parse_num("64", 40_u32), 64);
        assert_eq!(parse_num(" 64 ", 40_u32), 64);
        assert_eq!(parse_num("+64", 40_u32), 64);
    }

    #[test]
    fn the_ceiling_clamps_into_postgres_scale() {
        // 0 or a negative selection would refuse every acquire; 200 already
        // overruns a default postgres — both clamp to something servable.
        assert_eq!("0".parse::<u32>().unwrap().clamp(1, 200), 1);
        assert_eq!("9999".parse::<u32>().unwrap().clamp(1, 200), 200);
    }
}

// /api/healthz — liveness/readiness. Public by design (a health check that
// needs auth tells you nothing when auth is what's broken) and SAFE to
// expose (booleans, latencies and short machine error codes only — never
// connection strings, hostnames or driver messages).

use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::time::{Duration, Instant};

const PING_TIMEOUT: Duration = Duration::from_millis(2_500);

#[derive(serde::Serialize)]
pub struct Check {
    pub ok: bool,
    /// Null on failure — a check that didn't complete has no latency to
    /// report.
    #[serde(rename = "latencyMs")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(serde::Serialize)]
pub struct HealthBody {
    pub status: &'static str,
    #[serde(rename = "uptimeSeconds")]
    pub uptime_seconds: u64,
    pub checks: HealthChecks,
}

#[derive(serde::Serialize)]
pub struct HealthChecks {
    pub postgres: Check,
    pub redis: Check,
}

pub async fn get(State(state): State<AppState>) -> Response {
    // Both checks in parallel, each independently bounded: a wedged socket
    // must not hold the health check (or its probe) open.
    let (postgres, redis) = tokio::join!(
        timed("postgres", pg_ping(&state)),
        timed("redis", redis_ping(&state))
    );

    let ok = postgres.ok && redis.ok;
    let body = HealthBody {
        status: if ok { "ok" } else { "degraded" },
        uptime_seconds: state.uptime_seconds(),
        checks: HealthChecks { postgres, redis },
    };
    // 503 so a probe fails on its own, without parsing the body.
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

async fn pg_ping(state: &AppState) -> Result<(), sqlx::Error> {
    // A raw round trip on the pool — never anything that could migrate or
    // wait on the migration advisory lock.
    sqlx::query("select 1").execute(&state.pg).await.map(|_| ())
}

async fn redis_ping(state: &AppState) -> Result<(), redis::RedisError> {
    let mut conn = state.redis().await?;
    let _pong: String = redis::cmd("PING").query_async(&mut conn).await?;
    Ok(())
}

async fn timed<E, F>(name: &'static str, ping: F) -> Check
where
    E: std::error::Error + 'static,
    F: std::future::Future<Output = Result<(), E>>,
{
    let started = Instant::now();
    match tokio::time::timeout(PING_TIMEOUT, ping).await {
        Ok(Ok(())) => Check {
            ok: true,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(Err(e)) => {
            tracing::error!("[healthz] {name}: {e}");
            Check {
                ok: false,
                latency_ms: None,
                error: Some(safe_code(&e)),
            }
        }
        Err(_) => {
            tracing::error!("[healthz] {name}: TIMEOUT");
            Check {
                ok: false,
                latency_ms: None,
                error: Some("TIMEOUT".into()),
            }
        }
    }
}

/// Driver errors are not safe to echo. Keep the short machine code
/// (ECONNREFUSED, 28P01, TIMEOUT) and drop everything else — the full error
/// is logged above. The gate is `[A-Z0-9_]{1,20}`.
fn safe_code<E: std::error::Error + 'static>(e: &E) -> String {
    let as_dyn: &dyn std::error::Error = e;
    // sqlx database errors carry the server's SQLSTATE (uppercase
    // alphanumerics) — exactly the class the gate admits.
    if let Some(sqlx::Error::Database(dbe)) = as_dyn.downcast_ref::<sqlx::Error>()
        && let Some(code) = dbe.code()
        && (1..=20).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return code.to_string();
    }
    if e.to_string().contains("timeout") {
        return "TIMEOUT".into();
    }
    "unreachable".into()
}

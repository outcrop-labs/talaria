// The Postgres pool.
//
// THE SCHEMA IS MANAGED OUTSIDE THIS CRATE. This service reads and writes
// existing tables and never runs sqlx::migrate!, issues DDL, or writes to
// schema_migrations — the migration history is owned and applied elsewhere
// (advisory lock 8_314_207 guards it).

use crate::config::Config;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

/// Pool ceiling — 10 connections, comfortable against default postgres
/// limits.
pub const MAX_CONNECTIONS: u32 = 10;

pub fn pool(cfg: &Config) -> PgPool {
    // connect_lazy: construction does no I/O, so the api boots before
    // postgres and its healthz reports degraded instead of the process
    // dying — only healthz's round trip tells the truth.
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy(&cfg.database_url)
        .expect("DATABASE_URL passed Config's scheme check but sqlx rejects it")
}

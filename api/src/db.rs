// The Postgres pool.
//
// THE SCHEMA IS NOT OURS DURING COEXISTENCE. The TS server owns it: the
// MIGRATIONS array in ui/src/server/db/pg.ts is append-only (each statement's
// array index is its migration id, with a sha256 checksum that refuses to boot
// on any edit), applied at TS boot under advisory lock 8_314_207. This service
// reads and writes EXISTING tables and must never run sqlx::migrate!, issue
// DDL, or write to schema_migrations — until the TS server is retired
// (docs/RUST-MIGRATION.md, prod-cutover phase).

use crate::config::Config;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

/// Same shape as the TS pool (postgres-js, max 10): two runtimes sharing one
/// dev database sit at 20 total connections, comfortable against dev postgres
/// defaults. Revisit at prod cutover.
pub const MAX_CONNECTIONS: u32 = 10;

pub fn pool(cfg: &Config) -> PgPool {
    // connect_lazy: construction does no I/O, so the api boots before
    // postgres and its healthz reports degraded instead of the process
    // dying — the same posture as the TS side, where getSql() is lazy and
    // only healthz's round trip tells the truth.
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy(&cfg.database_url)
        .expect("DATABASE_URL passed Config's scheme check but sqlx rejects it")
}

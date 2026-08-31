// Path params. One gate lives here because three route files needed the same
// corner: TS binds {id} straight into raw SQL, so a non-uuid id is an uncaught
// bind error — the platform's plain-text 500 on TS, the house envelope here
// (the recorded platform-500 divergence, RUST-MIGRATION.md). Some(gate) is the
// response to return; the Option shape keeps clippy's large-Err lint quiet.

use crate::error::thrown_internal_error;
use axum::response::Response;
use uuid::Uuid;

pub fn uuid_gate(module: &str, action: &str, id: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    tracing::error!("[{module}] non-uuid id on {action}: {id:?}");
    Some(thrown_internal_error())
}

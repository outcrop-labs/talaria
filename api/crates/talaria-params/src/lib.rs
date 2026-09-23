// Path params. Binding {id} straight into `$N::uuid` makes a non-uuid id an
// uncaught Postgres error, which the route then reports as a 500. That is a
// client address, not a server fault — agents pass ticket refs, board names,
// and seqs, and a granted caller must not see Internal Server Error for one.
// Some(gate) is the response to return; the Option shape keeps clippy's
// large-Err lint quiet.

use axum::http::StatusCode;
use axum::response::Response;
use talaria_error::house_error;
use uuid::Uuid;

pub fn uuid_gate(module: &str, action: &str, id: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    tracing::warn!("[{module}] non-uuid id on {action}: {id:?}");
    Some(house_error(StatusCode::BAD_REQUEST, "not a uuid"))
}

/// The same shape, answering "not found" instead: some routes take an id an
/// AGENT supplied and bind it straight into a lookup, where a malformed id is
/// honestly a 404 rather than a server fault.
pub fn uuid_gate_404(id: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    Some(house_error(StatusCode::NOT_FOUND, "not found"))
}

// Path params. One gate lives here because three route files needed the same
// corner: binding {id} straight into raw SQL makes a non-uuid id an uncaught
// bind error, so the gate checks the shape first and answers with the house
// error envelope. Some(gate) is the
// response to return; the Option shape keeps clippy's large-Err lint quiet.

use axum::http::StatusCode;
use axum::response::Response;
use talaria_error::{house_error, internal};
use uuid::Uuid;

pub fn uuid_gate(module: &str, action: &str, id: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    Some(internal(
        &format!("[{module}] non-uuid id on {action}"),
        format!("{id:?}"),
    ))
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

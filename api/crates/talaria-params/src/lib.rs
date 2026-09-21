// Path params. One gate lives here because three route files needed the same
// corner: binding {id} straight into raw SQL makes a non-uuid id an uncaught
// bind error, so the gate checks the shape first and answers with the house
// error envelope. Some(gate) is the
// response to return; the Option shape keeps clippy's large-Err lint quiet.

use axum::response::Response;
use talaria_error::internal;
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

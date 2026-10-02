// POST /api/workbench/auth/v1/credential — refused, deliberately.
//
// In omp's protocol this is how a credential gets INTO a broker: a person runs
// `omp login` on the broker host and the result is uploaded. Talaria's logins
// happen in the UI, performed by this instance, and the only clients of this
// broker are agent sandboxes. A sandbox that could upload a credential could
// grant itself a subscription nobody signed it in to, so this answers 403 with
// that reason rather than accepting one.
//
// It is a route rather than a 404 so the refusal is legible in a harness log:
// "the broker exists and said no" beats "the broker has no such endpoint".

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use talaria_error::house_error;
use talaria_state::AppState;

use super::coding_broker::broker_agent;

// doc: Refused by design. Coding accounts are signed in by a person in
// doc: Talaria's UI, never uploaded from a sandbox — an agent that could write
// doc: its own credential could grant itself a subscription.
pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    // Authenticate first so an unauthenticated caller still gets a 401: the
    // refusal below is a statement about what agents may do, not an oracle
    // for whether this endpoint exists.
    broker_agent(&state, &headers).await?;
    Err(house_error(
        StatusCode::FORBIDDEN,
        "coding accounts are signed in from Talaria's UI, not uploaded from a sandbox",
    ))
}

// Route handlers, and THE router — built here (not in main.rs) so integration
// tests drive the exact same stack the process serves.

pub mod health;
pub mod llm_models;

use crate::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use std::time::Duration;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/healthz", get(health::get))
        .route("/api/llm/v1/models", get(llm_models::get))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        // 0.7 deprecated the 408-defaulting constructor: name the status we
        // actually want a timed-out JSON call to return.
        //
        // NOTE for the streaming phase: this bounds every route on the router.
        // When /api/llm/v1/chat/completions lands it must NOT sit under this
        // layer — a long SSE stream is a legitimate request (the upstream
        // ceiling is its own 10-minute budget, like UPSTREAM_TIMEOUT_MS in
        // llm-gateway.ts). Mount the streaming route on a timeout-free
        // sub-router instead of widening this.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            Duration::from_secs(30),
        ))
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

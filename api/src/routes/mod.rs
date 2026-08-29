// Route handlers, and THE router — built here (not in main.rs) so integration
// tests drive the exact same stack the process serves.

pub mod activity;
pub mod agents;
pub mod apps;
pub mod auth_claim;
pub mod auth_google;
pub mod auth_google_callback;
pub mod auth_logout;
pub mod auth_password;
pub mod auth_providers;
pub mod auth_session;
pub mod cost;
pub mod health;
pub mod llm_chat;
pub mod llm_models;
pub mod users;

use crate::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use std::time::Duration;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    // Two stacks, one reason: chat/completions is a STREAMING route whose
    // legitimate lifetime is bounded by the upstream's own 10-minute budget
    // (UPSTREAM_TIMEOUT_MS), not by a handler timeout. It must NOT sit under
    // the 30s TimeoutLayer the request/response routes use — widening that
    // layer for everyone would be the wrong trade.
    let timed = Router::new()
        .route("/api/healthz", get(health::get))
        .route("/api/llm/v1/models", get(llm_models::get))
        .route("/api/auth/session", get(auth_session::get))
        .route("/api/auth/logout", post(auth_logout::post))
        .route("/api/auth/password", post(auth_password::post))
        .route("/api/auth/providers", get(auth_providers::get))
        .route("/api/auth/claim", post(auth_claim::post))
        .route("/api/auth/google", get(auth_google::get))
        .route("/api/auth/google/callback", get(auth_google_callback::get))
        .route("/api/users", get(users::get))
        .route("/api/agents", get(agents::get))
        .route("/api/apps", get(apps::get))
        .route("/api/activity", get(activity::get))
        .route("/api/cost", get(cost::get))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        // 0.7 deprecated the 408-defaulting constructor: name the status we
        // actually want a timed-out JSON call to return.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            Duration::from_secs(30),
        ))
        .layer(CatchPanicLayer::new())
        .with_state(state.clone());

    let streaming = Router::new()
        .route("/api/llm/v1/chat/completions", post(llm_chat::post))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state);

    timed.merge(streaming)
}

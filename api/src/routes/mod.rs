// Route handlers, and THE router — built here (not in main.rs) so integration
// tests drive the exact same stack the process serves.

pub mod activity;
pub mod admin_google_client;
pub mod admin_instance;
pub mod admin_model_roles;
pub mod admin_password_accounts;
pub mod admin_permissions;
pub mod agent_role_templates;
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
pub mod keys;
pub mod keys_id;
pub mod llm_chat;
pub mod llm_models;
pub mod me;
pub mod models;
pub mod models_efforts;
pub mod notifications;
pub mod teams;
pub mod teams_id;
pub mod teams_id_members;
pub mod users;
pub mod workflows;
pub mod workflows_id;

use crate::state::AppState;
use axum::Router;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use std::time::Duration;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// app.ts's 405: the body is fixed, and the allow header is the TS route
/// file's handler keys in DECLARATION order (Object.keys of the handlers
/// object) — the order is pinned, not alphabetical.
fn method_not_allowed(allow: &'static str) -> Response {
    let mut res = crate::error::house_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    res.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    res
}

/// app.ts's API 404: `/api` and everything under `/api/` answer the JSON
/// sentence; anything else is not this server's to describe (the SPA shell is
/// a TS-side concern until cutover).
async fn api_not_found(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return crate::error::house_error(StatusCode::NOT_FOUND, "not found");
    }
    StatusCode::NOT_FOUND.into_response()
}

pub fn router(state: AppState) -> Router {
    // Two stacks, one reason: chat/completions is a STREAMING route whose
    // legitimate lifetime is bounded by the upstream's own 10-minute budget
    // (UPSTREAM_TIMEOUT_MS), not by a handler timeout. It must NOT sit under
    // the 30s TimeoutLayer the request/response routes use — widening that
    // layer for everyone would be the wrong trade.
    let timed = Router::new()
        .route(
            "/api/healthz",
            get(health::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/llm/v1/models",
            get(llm_models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/session",
            get(auth_session::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/logout",
            post(auth_logout::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/password",
            post(auth_password::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/providers",
            get(auth_providers::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/claim",
            post(auth_claim::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/google",
            get(auth_google::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/google/callback",
            get(auth_google_callback::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/users",
            get(users::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agents",
            get(agents::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/apps",
            get(apps::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/activity",
            get(activity::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models",
            get(models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models/efforts",
            get(models_efforts::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/cost",
            get(cost::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/keys",
            get(keys::get)
                .post(keys::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/keys/{id}",
            axum::routing::delete(keys_id::delete)
                .put(keys_id::put)
                .fallback(|| async { method_not_allowed("DELETE, PUT") }),
        )
        .route(
            "/api/teams",
            get(teams::get)
                .post(teams::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/teams/{id}",
            axum::routing::patch(teams_id::patch)
                .delete(teams_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/teams/{id}/members",
            get(teams_id_members::get)
                .post(teams_id_members::post)
                .delete(teams_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/workflows",
            get(workflows::get)
                .post(workflows::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/notifications",
            get(notifications::get)
                .put(notifications::put)
                .patch(notifications::patch)
                .fallback(|| async { method_not_allowed("GET, PUT, PATCH") }),
        )
        .route(
            "/api/me",
            get(me::get)
                .put(me::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workflows/{id}",
            put(workflows_id::put)
                .delete(workflows_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/agent-role-templates",
            get(agent_role_templates::get)
                .put(agent_role_templates::put)
                .delete(agent_role_templates::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/password-accounts",
            get(admin_password_accounts::get)
                .post(admin_password_accounts::post)
                .put(admin_password_accounts::put)
                .delete(admin_password_accounts::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client",
            get(admin_google_client::get)
                .put(admin_google_client::put)
                .delete(admin_google_client::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client/login",
            put(admin_google_client::put_login).fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/admin/instance",
            get(admin_instance::get)
                .put(admin_instance::put)
                .post(admin_instance::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/permissions",
            get(admin_permissions::get)
                .put(admin_permissions::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/model-roles",
            get(admin_model_roles::get)
                .put(admin_model_roles::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .fallback(api_not_found)
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
        .route(
            "/api/llm/v1/chat/completions",
            post(llm_chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state);

    timed.merge(streaming)
}

// talaria-api — the Rust successor to the TS fetch handler, taking over route
// groups one prefix at a time (docs/RUST-MIGRATION.md). Process shell only:
// config, tracing, pools, router, graceful shutdown. Everything observable
// lives in the route modules.

mod config;
mod db;
mod error;
mod ratelimit;
mod routes;
mod secretbox;
mod state;

use crate::config::Config;
use crate::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use std::sync::Arc;
use std::time::Duration;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() {
    // Fail once, with every problem named — config.rs is env.ts's philosophy.
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(problems) => {
            eprintln!("{problems}");
            std::process::exit(1);
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "talaria_api=debug,tower_http=info,info".into()),
        )
        .init();

    let root_source = cfg.secret_root.source();
    if root_source == config::RootSource::AuthSecretFallback {
        // Same warning secretbox.ts prints at boot: this database is sealed
        // with AUTH_SECRET, whose own documentation calls it safe to rotate.
        tracing::warn!(
            "[secretbox] encryption root is AUTH_SECRET (fallback), not TALARIA_SECRET_KEY — \
             rotating AUTH_SECRET makes every stored secret unrecoverable."
        );
    }

    let bind = cfg.bind;
    let state = AppState::new(db::pool(&cfg), Arc::new(cfg));

    // NOTE for the streaming phase: the global 30s timeout below bounds THIS
    // router only. When /api/llm/v1/chat/completions lands it must mount on a
    // router without a total-request timeout (a long SSE stream is a legitimate
    // request; the upstream ceiling is its own 10-minute budget, like
    // UPSTREAM_TIMEOUT_MS in llm-gateway.ts) — don't widen this layer to it.
    let app = Router::new()
        .route("/api/healthz", get(routes::health::get))
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

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|e| panic!("cannot bind {bind}: {e}"));
    tracing::info!("talaria-api listening on http://{bind} (migration coexistence service)");

    // Graceful shutdown on SIGTERM (compose/systemd) and Ctrl-C (dev): stop
    // accepting, let in-flight requests finish, then drain the pool.
    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());
    match serve.await {
        Ok(()) => tracing::info!("shutdown complete"),
        Err(e) => {
            tracing::error!("server error: {e}");
            state.pg.close().await;
            std::process::exit(1);
        }
    }
    state.pg.close().await;
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => sig.recv().await, // Option<()>: None when the stream ends
            Err(_) => std::future::pending::<Option<()>>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<Option<()>>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received — draining");
}

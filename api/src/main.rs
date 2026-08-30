// talaria-api — the Rust successor to the TS fetch handler, taking over route
// groups one prefix at a time (docs/RUST-MIGRATION.md). Process shell only:
// config, tracing, pools, router, graceful shutdown. Everything observable
// lives in the library (src/lib.rs) — the router itself in routes::router, so
// integration tests drive the exact stack this serves.

use std::sync::Arc;
use talaria_api::{config, config::Config, db, jobs, routes, scheduler, state::AppState};

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

    // THE FLIP. `TALARIA_SCHEDULER=rust` moves the whole job schedule into
    // this process: TS's startScheduler reads the same value and stands down,
    // so one env declaration hands the schedule over, never a window where
    // both runtimes arm. `arm` retries until Postgres and Redis answer — boot
    // itself stays independent of both, exactly like TS's.
    if scheduler::rust_owns_schedule() {
        let st = state.clone();
        tokio::spawn(async move { jobs::arm(st).await });
    }

    let app = routes::router(state.clone());

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
    // When the schedule is this process's, draining means its drain too: no
    // new runs armed, in-flight job work given its grace, then the pool. A
    // job that ARCHIVES conversations or MESSAGES people must not be killed
    // half a second from either.
    if scheduler::rust_owns_schedule() {
        scheduler::stop_scheduler(30_000).await;
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

// talaria-api — the api: every /api/* route except the four permanent TS
// residents (the app modules, which stay TS). Process shell
// only: config, tracing, pools, router, graceful shutdown. Everything
// observable lives in the library (src/lib.rs) — the router itself in
// routes::router, so integration tests drive the exact stack this serves.

use std::sync::Arc;
use talaria_api::{config, config::Config, db, jobs, routes, scheduler, state::AppState};

#[tokio::main]
async fn main() {
    // Fail once, with every problem named.
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
        // This database is sealed with AUTH_SECRET, whose own documentation
        // calls it safe to rotate.
        tracing::warn!(
            "[secretbox] encryption root is AUTH_SECRET (fallback), not TALARIA_SECRET_KEY — \
             rotating AUTH_SECRET makes every stored secret unrecoverable."
        );
    }

    let bind = cfg.bind;
    let state = AppState::new(db::pool(&cfg), Arc::new(cfg));

    // The schedule is this process's — boot arms
    // it unless the kill switch is set (`TALARIA_SCHEDULER=off`; see the
    // arming-switch header in scheduler.rs). `arm` retries until Postgres
    // and Redis answer — boot itself stays independent of both. The off
    // posture logs HERE: nothing downstream of the gate runs, so nothing
    // else could say why the schedule is silent.
    if scheduler::scheduler_disabled() {
        tracing::warn!(
            "[scheduler] disabled by TALARIA_SCHEDULER=off — no background jobs will run on this instance"
        );
    } else {
        let st = state.clone();
        tokio::spawn(async move { jobs::arm(st).await });
    }

    // The builtin MCP rows (the toolkit, the Workbench) are this api's to
    // guarantee: every reader — the gateway, agents' rendered configs, the
    // SPA harness's capability reach — treats them as present, so boot seeds
    // them rather than waiting for the first admin list. Best-effort, like
    // arm: a database still coming up retries on the next registry list,
    // which re-ensures.
    {
        let pg = state.pg.clone();
        tokio::spawn(async move {
            if let Err(e) = talaria_api::mcp::registry::ensure_builtin_mcp(&pg).await {
                tracing::warn!(
                    "[mcp] builtin rows not seeded — the first registry list retries: {e}"
                );
            }
        });
    }

    let app = routes::router(state.clone());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|e| panic!("cannot bind {bind}: {e}"));
    tracing::info!("talaria-api listening on http://{bind}");

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
    // Draining means the scheduler's drain too: no new runs armed, in-flight
    // job work given its grace, then the pool. A job that ARCHIVES
    // conversations or MESSAGES people must not be killed half a second from
    // either. (No-op when nothing was armed — the kill switch, or a boot
    // whose arm loop never finished.)
    scheduler::stop_scheduler(30_000).await;
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

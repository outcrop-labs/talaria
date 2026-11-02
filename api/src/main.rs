// talaria-api — the api: every /api/* route except the four permanent TS
// residents (the app modules, which stay TS). Process shell
// only: config, tracing, pools, router, graceful shutdown. Everything
// observable lives in the library (src/lib.rs) — the router itself in
// routes::router, so integration tests drive the exact stack this serves.

use std::sync::Arc;
use talaria_api::{config, config::Config, db, jobs, scheduler, state::AppState};

fn main() {
    // THE HIDDEN MODE, before anything else — no runtime, no config, no
    // tracing. `talaria-api __code-probe` is this same binary re-executed by
    // the model-fitness code probe to evaluate model-written JavaScript in a
    // process of its own, under rlimits, so a runaway candidate costs that
    // process and never the api. Dispatched here, ahead of the tokio
    // runtime, so the child's address-space ceiling is not spent on worker
    // threads it will never use.
    if std::env::args().nth(1).as_deref() == Some(talaria_fitness_code_runner::CHILD_PROBE_ARG) {
        std::process::exit(talaria_fitness_code_runner::child_main());
    }
    // And the other half: this binary answers that mode, so the probe may
    // use it. Unset in every other process (tests, tools), where grading
    // stays in-process.
    if let Ok(exe) = std::env::current_exe() {
        let _ = talaria_fitness_code_runner::CHILD_PROBE.set(exe);
    }
    serve();
}

#[tokio::main]
async fn serve() {
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
            if let Err(e) = talaria_mcp::registry::ensure_builtin_mcp(&pg).await {
                tracing::warn!(
                    "[mcp] builtin rows not seeded — the first registry list retries: {e}"
                );
            }
        });
    }

    // Drop apps whose last compile/load failed. Enablement already probes;
    // this closes the window after a crash between boots. GET /admin/apps
    // is a read — it must not mutate the enabled set. Then reconcile the
    // app MCP rows UNCONDITIONALLY: the app host turns a broken app off by
    // writing the enabled set itself, and a host that did so while this
    // process was down left the app's server serving on the agents carrying
    // it. The sync turns such a row off (keeping its grants), and copies
    // every serving row's catalog fresh from its build.
    {
        let state = state.clone();
        tokio::spawn(async move {
            if let Ok(sb) = state.secretbox().await {
                talaria_api::apps::disable_broken_apps(&state.pg, &sb).await;
                // An app that is off holds no gateway key, whoever turned it
                // off and whenever.
                if let Err(e) = talaria_api::apps::reconcile_app_llm_keys(&state.pg).await {
                    tracing::warn!("[apps] boot gateway-key reconcile failed: {e}");
                }
                if let Err(e) = talaria_api::apps::sync_app_mcp_servers(&state.pg, &sb).await {
                    tracing::warn!("[apps] boot MCP registry sync failed: {e}");
                }
            }
        });
    }

    // Package-run MCP servers (marketplace installs from the registry's
    // npm/pypi/oci long tail) get the same boot guarantee: a reconcile pass
    // removes strays a previous life left and containers disabled rows no
    // longer want. Enabled oci-http containers self-heal on first use.
    {
        let pg = state.pg.clone();
        tokio::spawn(async move {
            talaria_mcp::pkg::reconcile(&pg).await;
        });
    }

    // The toolkit child gets the same boot guarantee. It spawns on demand —
    // renders and comms reads are the only callers that summon it — so a
    // deploy's first agent session can beat the spawn, and a session whose
    // initialize fails drops the server (every talaria tool with it) for its
    // whole lifetime. Boot closes the window before any session opens it.
    // Fire-and-forget like everything here; the gateway relay double-checks
    // before every builtin hop.
    talaria_mcp_service::ensure_mcp_service();

    let app = talaria_api_routes::routes::router(state.clone());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|e| panic!("cannot bind {bind}: {e}"));
    tracing::info!("talaria-api listening on http://{bind}");

    // Graceful shutdown on SIGTERM (compose/systemd) and Ctrl-C (dev): stop
    // accepting, let in-flight requests finish — for at most HTTP_DRAIN_MS
    // after the signal — then drain the runs, the scheduler and the pool.
    // Each phase logs how long it took, so a stop that lingers names what
    // it waited on.
    let signalled = Arc::new(tokio::sync::Notify::new());
    let serve =
        axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(signalled.clone()));
    let drain_bound = async {
        signalled.notified().await;
        tokio::time::sleep(std::time::Duration::from_millis(HTTP_DRAIN_MS)).await;
    };
    let started = std::time::Instant::now();
    tokio::select! {
        served = serve => match served {
            Ok(()) => tracing::info!(
                "[shutdown] http drained in {}ms — every response finished",
                started.elapsed().as_millis().saturating_sub(signal_ms(&started))
            ),
            Err(e) => {
                tracing::error!("server error: {e}");
                state.pg.close().await;
                std::process::exit(1);
            }
        },
        // A response that is not an event stream (those end on the signal)
        // and is still running past the bound — a long model call, a slow
        // upload — is left to the exit rather than holding the stop open
        // until the container's own kill.
        () = drain_bound => tracing::warn!(
            "[shutdown] responses still in flight {HTTP_DRAIN_MS}ms after the signal — not waiting for them"
        ),
    }
    // THE RUN DRIVERS, before anything else: abort every live step and let
    // each drive's cleanup release its lease, so the instance that replaces
    // this one resumes the work in seconds (its 30s sweep) rather than after
    // each lease TTL — a roll must never stall work it can hand over.
    let phase = std::time::Instant::now();
    talaria_runs_drivers::drain(5_000).await;
    tracing::info!(
        "[shutdown] run drivers drained in {}ms",
        phase.elapsed().as_millis()
    );
    // Draining means the scheduler's drain too: no new runs armed, in-flight
    // job work given its grace, then the pool. A job that ARCHIVES
    // conversations or MESSAGES people must not be killed half a second from
    // either. (No-op when nothing was armed — the kill switch, or a boot
    // whose arm loop never finished.) It logs the jobs it waits for.
    let phase = std::time::Instant::now();
    scheduler::stop_scheduler(30_000).await;
    tracing::info!(
        "[shutdown] scheduler stopped in {}ms",
        phase.elapsed().as_millis()
    );
    // THE POOL, bounded too. `close` waits for every checked-out connection
    // to come back, and a detached task's query is not drained by anything
    // above — measured: the boot seed of the builtin MCP rows, an insert held
    // behind a slow disk's commit, kept the process alive 11-35 s after every
    // stream, run and job had finished. Past the bound the stop goes on: the
    // query's connection drops with the process, as the container's own kill
    // would drop it, and Postgres keeps the statement atomic either way.
    let phase = std::time::Instant::now();
    let closed = tokio::time::timeout(
        std::time::Duration::from_millis(POOL_CLOSE_MS),
        state.pg.close(),
    )
    .await
    .is_ok();
    let after_signal = started
        .elapsed()
        .as_millis()
        .saturating_sub(signal_ms(&started));
    if closed {
        tracing::info!(
            "[shutdown] complete — pool closed in {}ms, {after_signal}ms after the signal",
            phase.elapsed().as_millis()
        );
    } else {
        tracing::warn!(
            "[shutdown] complete — {} pool connection(s) still running a query {POOL_CLOSE_MS}ms on (a detached task's, after every drain); not waiting for them. {after_signal}ms after the signal",
            (state.pg.size() as usize).saturating_sub(state.pg.num_idle())
        );
    }
}

/// How long the pool gets to take its connections back once everything that
/// is drained on purpose has been.
const POOL_CLOSE_MS: u64 = 1_000;

/// How long in-flight responses get after the shutdown signal. Event streams
/// end at the signal itself (`end_streams`); this bounds everything else.
const HTTP_DRAIN_MS: u64 = 5_000;

/// When the signal landed, in ms after `since` (0 before it has).
static SIGNAL_AT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn signal_ms(since: &std::time::Instant) -> u128 {
    SIGNAL_AT
        .get()
        .map_or(0, |at| at.saturating_duration_since(*since).as_millis())
}

async fn shutdown_signal(signalled: Arc<tokio::sync::Notify>) {
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
    let _ = SIGNAL_AT.set(std::time::Instant::now());
    tracing::info!("shutdown signal received — draining");
    // Event streams never finish on their own, and the graceful drain waits
    // on every response: end them now, so the drain is not held open by the
    // browsers that are connected (each reconnects after its `retry:`). The
    // realtime hub closes with them — its subscriber connection, and every
    // stream a route built on it by hand.
    talaria_realtime::end_streams();
    signalled.notify_one();
}
